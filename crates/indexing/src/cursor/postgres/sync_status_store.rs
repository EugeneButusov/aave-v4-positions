//! The indexer's position, in Postgres.
//!
//! **Writes nothing.** It shares a table with the cursor store rather than
//! duplicating one, because there is exactly one answer to "where is the
//! indexer" and a second copy of it would be a second thing to keep in step.

use postgres::{Pool, connection};
use time::OffsetDateTime;
use tokio_postgres::Row;

use crate::cursor::{Error, SyncStatus, SyncStatusStore};

/// **`age_seconds` is computed here, not by the reader.** `updated_at` is the
/// database's own `now()`, so a reader subtracting it from its own clock
/// reports skew as staleness — and a fast reader would call a healthy indexer
/// stale. Floored to whole seconds: `EXTRACT` returns microseconds, and six
/// decimal places on a figure compared against a threshold in tens of seconds
/// is precision nobody can use and everybody has to read.
const STATUS: &str = "\
    SELECT \
        last_block, \
        last_hash, \
        updated_at, \
        floor(EXTRACT(EPOCH FROM (now() - updated_at)))::bigint AS age_seconds \
    FROM indexer_cursor \
    WHERE chain_id = $1 \
    LIMIT 1";

pub struct PostgresSyncStatusStore {
    pool: Pool,
}

impl PostgresSyncStatusStore {
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl SyncStatusStore for PostgresSyncStatusStore {
    async fn get(&self, chain_id: u32) -> Result<Option<SyncStatus>, Error> {
        let client = connection(&self.pool).await?;
        let row = client.query_opt(STATUS, &[&i64::from(chain_id)]).await?;

        row.as_ref().map(|row| status(chain_id, row)).transpose()
    }
}

fn status(chain_id: u32, row: &Row) -> Result<SyncStatus, Error> {
    let last_hash: &str = row.get("last_hash");
    let last_block: i64 = row.get("last_block");
    let age_seconds: i64 = row.get("age_seconds");

    Ok(SyncStatus {
        // The argument, not the column. `WHERE chain_id = $1` matched it, so
        // the row cannot disagree — and reading it back would be a `bigint` to
        // narrow with nothing sensible to do if it did not fit.
        chain_id,
        // `CHECK (last_block >= 0)` puts this out of reach. It refuses rather
        // than substitutes anyway: the substitute would be 0, which reads as a
        // chain indexed to genesis, and every payload is stamped with this.
        last_block: last_block.try_into().map_err(|_| Error::Malformed {
            column: "last_block",
            expected: "non-negative",
            value: last_block.to_string(),
        })?,
        last_hash: last_hash.parse().map_err(|_| Error::Malformed {
            column: "last_hash",
            expected: "32-byte hash",
            value: last_hash.to_owned(),
        })?,
        updated_at: row.get::<_, OffsetDateTime>("updated_at"),
        // Clamped rather than refused, and the only one of these that is.
        // Negative means the row was written by a clock ahead of the one
        // reading it, and "not stale" is the honest answer to that — unlike a
        // height or a hash, where no answer is better than a plausible one.
        age_seconds: age_seconds.try_into().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    //! The port's specification, against a real Postgres.

    use crate::cursor::conformance;
    use crate::cursor::postgres::harness::PostgresHarness;

    macro_rules! conformance {
        ($($case:ident),* $(,)?) => {
            $(
                #[tokio::test]
                async fn $case() {
                    conformance::$case::<PostgresHarness>().await;
                }
            )*
        };
    }

    conformance! {
        reads_the_position_the_indexer_left,
        answers_none_for_a_chain_never_indexed,
        reads_only_the_chain_it_was_asked_about,
        ages_the_row_by_the_database_clock,
        reads_a_block_height_past_what_a_double_can_hold,
    }
}
