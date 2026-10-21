//! The cursor, in the database the indexer keeps its own state in.

use postgres::{Pool, TracedPool};
use tokio_postgres::Row;

use crate::cursor::{Cursor, CursorStore, Error};

const LOAD: &str = "\
    SELECT last_block, last_hash \
    FROM indexer_cursor \
    WHERE chain_id = $1 \
    LIMIT 1";

/// **No monotonic guard, deliberately.** `ON CONFLICT … WHERE last_block <
/// EXCLUDED.last_block` is the integrity check this looks like it is missing,
/// and it would break every reorg: unwinding one saves the last *valid* block,
/// below where the cursor already stood, so the guarded write would report
/// success and silently discard the rewind.
///
/// `updated_at` is this server's `now()`, because that is the clock
/// `SyncStatusStore` ages the row against.
const SAVE: &str = "\
    INSERT INTO indexer_cursor (chain_id, last_block, last_hash, updated_at) \
    VALUES ($1, $2, $3, now()) \
    ON CONFLICT (chain_id) DO UPDATE \
       SET last_block = EXCLUDED.last_block, \
           last_hash  = EXCLUDED.last_hash, \
           updated_at = EXCLUDED.updated_at";

pub struct PostgresCursorStore {
    db: TracedPool,
}

impl PostgresCursorStore {
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self {
            db: TracedPool::new(pool),
        }
    }
}

#[async_trait::async_trait]
impl CursorStore for PostgresCursorStore {
    async fn load(&self, chain_id: u32) -> Result<Option<Cursor>, Error> {
        let row = self
            .db
            .query_opt("SELECT indexer_cursor", LOAD, &[&i64::from(chain_id)])
            .await?;

        row.as_ref().map(|row| cursor(chain_id, row)).transpose()
    }

    async fn save(&self, cursor: &Cursor) -> Result<(), Error> {
        self.db
            .execute(
                "INSERT indexer_cursor",
                SAVE,
                &[
                    &i64::from(cursor.chain_id),
                    &block(cursor.last_block),
                    &lower(cursor.last_hash),
                ],
            )
            .await?;

        Ok(())
    }
}

fn cursor(chain_id: u32, row: &Row) -> Result<Cursor, Error> {
    // The argument, not the column: `WHERE chain_id = $1` matched it.
    held(chain_id, row.get("last_block"), row.get("last_hash"))
}

/// The two columns as the driver hands them over, separate from the row so both
/// refusals are reachable — the table's `CHECK`s put them out of reach through
/// it.
fn held(chain_id: u32, last_block: i64, last_hash: &str) -> Result<Cursor, Error> {
    Ok(Cursor {
        chain_id,
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
    })
}

/// `bigint` is signed and block heights are not, so the wrap is unreachable
/// below 2^63 and the cast is the honest way to say so.
#[expect(
    clippy::cast_possible_wrap,
    reason = "a block height, which `bigint` is signed and blocks are not"
)]
fn block(number: u64) -> i64 {
    number as i64
}

/// The spelling the table's `CHECK (last_hash ~ '^0x[0-9a-f]{64}$')` demands.
fn lower(hash: alloy_primitives::B256) -> String {
    format!("{hash:#x}")
}

#[cfg(test)]
mod tests {
    //! The port's specification, against a real Postgres.

    use crate::cursor::conformance::cursor_store_conformance;
    use crate::cursor::postgres::harness::PostgresCursorHarness;

    cursor_store_conformance!(PostgresCursorHarness);
}

#[cfg(test)]
mod beyond_the_contract {
    //! What the port does not promise and this table must still get right.

    use crate::cursor::conformance::CursorHarness as _;
    use crate::cursor::postgres::harness::PostgresCursorHarness;
    use crate::cursor::{Cursor, CursorStore, SyncStatusStore};
    use crate::fixtures::{CHAIN_ID, hash_of};

    fn at(last_block: u64) -> Cursor {
        Cursor {
            chain_id: CHAIN_ID,
            last_block,
            last_hash: hash_of(b'a', last_block),
        }
    }

    #[tokio::test]
    async fn holds_one_row_per_chain_however_often_it_advances() {
        let harness = PostgresCursorHarness::fresh("one_row_per_chain").await;

        for block in 500..=503 {
            harness.store().save(&at(block)).await.unwrap();
        }

        assert_eq!(harness.rows().await, 1);
    }

    /// The write and the read are two adapters over one row, and `age_seconds`
    /// is computed from the clock that wrote it. A save that left `updated_at`
    /// alone would leave every payload the API stamps reporting a staleness
    /// from whenever this chain was first indexed.
    #[tokio::test]
    async fn an_advance_restamps_the_clock_the_read_ages_against() {
        let harness = PostgresCursorHarness::fresh("restamps_the_clock").await;

        harness.store().save(&at(500)).await.unwrap();
        let status = harness
            .as_read()
            .get(CHAIN_ID)
            .await
            .unwrap()
            .expect("the chain is indexed");

        assert_eq!(status.last_block, 500);
        assert!(status.age_seconds < 5, "{}", status.age_seconds);
    }
}

#[cfg(test)]
mod refusals {
    //! What a cursor read does with a column holding what its type says it
    //! cannot. Unreachable through the table, and decided here all the same.

    use super::held;
    use crate::cursor::Error;
    use crate::fixtures::{CHAIN_ID, hash_of};
    use crate::scratch::lower;

    fn column(error: Error) -> &'static str {
        match error {
            Error::Malformed { column, .. } => column,
            other => panic!("expected a malformed column: {other:?}"),
        }
    }

    /// It refuses rather than substituting, and the substitute is why: 0 reads
    /// as a chain indexed to genesis, and every payload is stamped with this.
    #[test]
    fn refuses_a_negative_height() {
        let refusal = held(CHAIN_ID, -1, &lower(hash_of(b'a', 500))).unwrap_err();

        assert_eq!(column(refusal), "last_block");
    }

    #[test]
    fn refuses_a_hash_it_cannot_read() {
        let refusal = held(CHAIN_ID, 500, "0xnope").unwrap_err();

        assert_eq!(column(refusal), "last_hash");
    }
}
