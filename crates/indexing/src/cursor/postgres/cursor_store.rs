//! The cursor, in the database the indexer keeps its own state in.

use postgres::{Bigint, Pool, TracedPool};
use tokio_postgres::Row;
use units::ChainId;

use crate::cursor::{Cursor, CursorStore, Error};

const LOAD: &str = "\
    SELECT last_block, last_hash \
    FROM indexer_cursor \
    WHERE chain_id = $1 \
    LIMIT 1";

/// **No `WHERE last_block < EXCLUDED.last_block`**, for the reason
/// [`CursorStore::save`] gives: it would discard a rewind and report success.
///
/// `updated_at` is this server's `now()`, the clock `SyncStatusStore` ages the
/// row against.
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
    async fn load(&self, chain_id: ChainId) -> Result<Option<Cursor>, Error> {
        let row = self
            .db
            .query_opt("SELECT indexer_cursor", LOAD, &[&Bigint(chain_id.into())])
            .await
            .map_err(Error::ReadFailed)?;

        row.as_ref().map(|row| cursor(chain_id, row)).transpose()
    }

    async fn save(&self, cursor: &Cursor) -> Result<(), Error> {
        self.db
            .execute(
                "INSERT indexer_cursor",
                SAVE,
                &[
                    &Bigint(cursor.chain_id.into()),
                    &Bigint(cursor.last_block),
                    &cursor.last_hash.to_string(),
                ],
            )
            .await
            .map_err(Error::WriteFailed)?;

        Ok(())
    }
}

fn cursor(chain_id: ChainId, row: &Row) -> Result<Cursor, Error> {
    // The argument, not the column: `WHERE chain_id = $1` matched it.
    held(chain_id, row.get("last_block"), row.get("last_hash"))
}

/// Separate from the `Row` so both refusals are reachable: the table's `CHECK`s
/// put them out of reach through it.
fn held(chain_id: ChainId, last_block: i64, last_hash: &str) -> Result<Cursor, Error> {
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

#[cfg(test)]
mod tests {
    //! The port's specification, against a real Postgres.

    use crate::cursor::conformance::cursor_store_conformance;
    use crate::cursor::postgres::harness::PostgresHarness;

    cursor_store_conformance!(PostgresHarness);
}

#[cfg(test)]
mod beyond_the_contract {
    //! What the port does not promise and this table must still get right.

    use crate::cursor::postgres::harness::PostgresHarness;
    use crate::cursor::{Cursor, CursorStore, SyncStatusStore};
    use crate::fixtures::{CHAIN_ID, WIDE_CHAIN, hash_of};

    fn at(last_block: u64) -> Cursor {
        Cursor {
            chain_id: CHAIN_ID,
            last_block,
            last_hash: hash_of(b'a', last_block),
        }
    }

    #[tokio::test]
    async fn holds_one_row_per_chain_however_often_it_advances() {
        let harness = PostgresHarness::over("one_row_per_chain").await;

        for block in 500..=503 {
            harness.cursors().save(&at(block)).await.unwrap();
        }

        assert_eq!(harness.rows().await, 1);
    }

    /// Through the read adapter, not this one: a round trip cannot see a
    /// narrowing applied to both of its halves.
    #[tokio::test]
    async fn stores_a_chain_id_past_what_a_u32_can_hold() {
        let harness = PostgresHarness::over("a_chain_id_past_a_u32").await;

        harness
            .cursors()
            .save(&Cursor {
                chain_id: WIDE_CHAIN,
                last_block: 500,
                last_hash: hash_of(b'a', 500),
            })
            .await
            .unwrap();

        let status = harness.sync().get(WIDE_CHAIN).await.unwrap();

        assert_eq!(status.map(|held| held.last_block), Some(500));
    }

    /// A save that left `updated_at` alone would have every payload the API
    /// stamps report a staleness from whenever this chain was first indexed.
    #[tokio::test]
    async fn an_advance_restamps_the_clock_the_read_ages_against() {
        let harness = PostgresHarness::over("restamps_the_clock").await;

        harness.cursors().save(&at(500)).await.unwrap();
        let status = harness
            .sync()
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
    //! Which refusal each failure is reported as: the direction a database
    //! failure happened in, and the column that held what its type forbids.

    use postgres::{Limits, build_pool};

    use super::{PostgresCursorStore, held};
    use crate::cursor::{Cursor, CursorStore, Error};
    use crate::fixtures::{CHAIN_ID, hash_of};

    /// A port nothing listens on, so no server is needed and the pool's
    /// `create` deadline is never reached.
    const NOWHERE: &str = "postgres://postgres@127.0.0.1:1/nothing";

    /// The two carry the same cause, so swapping them is invisible to every
    /// other case and to the message an operator reads.
    #[tokio::test]
    async fn says_which_direction_a_database_failure_was_in() {
        let store = PostgresCursorStore::new(build_pool(NOWHERE, Limits::default()).unwrap());
        let cursor = Cursor {
            chain_id: CHAIN_ID,
            last_block: 500,
            last_hash: hash_of(b'a', 500),
        };

        assert!(matches!(
            store.load(CHAIN_ID).await,
            Err(Error::ReadFailed(_))
        ));
        assert!(matches!(
            store.save(&cursor).await,
            Err(Error::WriteFailed(_))
        ));
    }

    fn column(error: Error) -> &'static str {
        match error {
            Error::Malformed { column, .. } => column,
            other => panic!("expected a malformed column: {other:?}"),
        }
    }

    /// The substitute is why it refuses: 0 reads as a chain indexed to genesis,
    /// and every payload is stamped with this.
    #[test]
    fn refuses_a_negative_height() {
        let refusal = held(CHAIN_ID, -1, &hash_of(b'a', 500).to_string()).unwrap_err();

        assert_eq!(column(refusal), "last_block");
    }

    #[test]
    fn refuses_a_hash_it_cannot_read() {
        let refusal = held(CHAIN_ID, 500, "0xnope").unwrap_err();

        assert_eq!(column(refusal), "last_hash");
    }
}
