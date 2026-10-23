use alloy_primitives::B256;
use postgres::{Bigint, Pool, TracedPool};
use tokio_postgres::Row;
use units::ChainId;

use crate::chain::BlockHeader;
use crate::reorg::{BlockHeaderStore, Error};

/// No `ORDER BY`: the port promises none, so adding one here would be a
/// guarantee a later caller leans on without it ever having been made.
const LOAD: &str = "\
    SELECT block_number, hash, parent_hash, block_timestamp \
    FROM indexer_block_headers \
    WHERE chain_id = $1";

/// A data-modifying CTE, so the upsert and the prune are one round trip and one
/// implicit transaction.
///
/// **`AND chain_id` on the DELETE is load-bearing.** Without it one chain's
/// retention floor wipes every other chain's window, and no single-chain case
/// can see the difference.
///
/// The DELETE reads the pre-INSERT snapshot, so the row just written is never a
/// prune candidate — harmless under the port's `retain_from <= header.number`.
const APPEND: &str = "\
    WITH retained AS ( \
        INSERT INTO indexer_block_headers \
            (chain_id, block_number, hash, parent_hash, block_timestamp) \
        VALUES ($1, $2, $3, $4, $5) \
        ON CONFLICT (chain_id, block_number) DO UPDATE \
           SET hash            = EXCLUDED.hash, \
               parent_hash     = EXCLUDED.parent_hash, \
               block_timestamp = EXCLUDED.block_timestamp \
    ) \
    DELETE FROM indexer_block_headers \
    WHERE chain_id = $1 AND block_number < $6";

const TRUNCATE: &str = "\
    DELETE FROM indexer_block_headers \
    WHERE chain_id = $1 AND block_number > $2";

/// Clears the window: every row satisfies `block_number > -1`, and the column's
/// `CHECK (block_number >= 0)` is what makes that exhaustive.
const CLEAR: &str = "DELETE FROM indexer_block_headers WHERE chain_id = $1";

pub struct PostgresBlockHeaderStore {
    db: TracedPool,
}

impl PostgresBlockHeaderStore {
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self {
            db: TracedPool::new(pool),
        }
    }
}

#[async_trait::async_trait]
impl BlockHeaderStore for PostgresBlockHeaderStore {
    async fn load(&self, chain_id: ChainId) -> Result<Vec<BlockHeader>, Error> {
        self.db
            .query(
                "SELECT indexer_block_headers",
                LOAD,
                &[&Bigint(chain_id.into())],
            )
            .await
            .map_err(Error::WindowReadFailed)?
            .iter()
            .map(header)
            .collect()
    }

    async fn append(
        &self,
        chain_id: ChainId,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        self.db
            .execute(
                "INSERT indexer_block_headers",
                APPEND,
                &[
                    &Bigint(chain_id.into()),
                    &Bigint(header.number),
                    &header.hash.to_string(),
                    &header.parent_hash.to_string(),
                    &Bigint(header.timestamp),
                    &Bigint(retain_from),
                ],
            )
            .await
            .map_err(Error::WindowWriteFailed)?;

        Ok(())
    }

    async fn truncate(
        &self,
        chain_id: ChainId,
        last_valid_block: Option<u64>,
    ) -> Result<(), Error> {
        // A separate statement rather than passing -1, because the column is
        // unsigned in every sense that matters and a sentinel that only works
        // while `bigint` is signed is a sentinel waiting to stop working.
        match last_valid_block {
            Some(block) => {
                self.db
                    .execute(
                        "DELETE indexer_block_headers",
                        TRUNCATE,
                        &[&Bigint(chain_id.into()), &Bigint(block)],
                    )
                    .await
                    .map_err(Error::WindowWriteFailed)?;
            }
            None => {
                self.db
                    .execute(
                        "DELETE indexer_block_headers",
                        CLEAR,
                        &[&Bigint(chain_id.into())],
                    )
                    .await
                    .map_err(Error::WindowWriteFailed)?;
            }
        }

        Ok(())
    }
}

fn header(row: &Row) -> Result<BlockHeader, Error> {
    retained(
        row.get("block_number"),
        row.get("hash"),
        row.get("parent_hash"),
        row.get("block_timestamp"),
    )
}

/// Separate from the `Row` so every refusal below is reachable: the table's own
/// `CHECK`s put all of them out of reach through it.
fn retained(
    number: i64,
    hash: &str,
    parent_hash: &str,
    timestamp: i64,
) -> Result<BlockHeader, Error> {
    Ok(BlockHeader {
        number: number.try_into().map_err(|_| Error::Malformed {
            column: "block_number",
            expected: "non-negative",
            value: number.to_string(),
        })?,
        hash: parse(hash, "hash")?,
        parent_hash: parse(parent_hash, "parent_hash")?,
        // The only one of these that is clamped rather than refused: nothing
        // here compares a timestamp against a clock.
        timestamp: timestamp.try_into().unwrap_or(0),
    })
}

fn parse(value: &str, column: &'static str) -> Result<B256, Error> {
    value.parse().map_err(|_| Error::Malformed {
        column,
        expected: "32-byte hash",
        value: value.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    //! The port's specification, against a real Postgres.

    use crate::reorg::conformance::block_header_store_conformance;
    use crate::reorg::postgres::harness::PostgresWindowHarness;

    block_header_store_conformance!(PostgresWindowHarness);
}

#[cfg(test)]
mod beyond_the_contract {
    //! What the port does not promise and this table must still get right.

    use crate::fixtures::{CHAIN_ID, WIDE_CHAIN, hash_of, header};
    use crate::reorg::BlockHeaderStore;
    use crate::reorg::conformance::Harness as _;
    use crate::reorg::postgres::harness::PostgresWindowHarness;

    /// Against the table, because `load` would hand two rows back as a window
    /// of two and the ancestor walk would read one of them as canonical.
    #[tokio::test]
    async fn holds_one_row_however_often_a_height_is_rewritten() {
        let harness = PostgresWindowHarness::fresh("holds_one_row_per_height").await;

        for branch in [b'a', b'b', b'c'] {
            harness
                .store()
                .append(CHAIN_ID, &header(500, branch), 0)
                .await
                .unwrap();
        }

        assert_eq!(harness.rows_at(CHAIN_ID, 500).await, 1);
    }

    /// Against the table, not through `load`: a round trip addresses the column
    /// at the width the write used, so it cannot see a narrowing of both.
    #[tokio::test]
    async fn stores_a_chain_id_past_what_a_u32_can_hold() {
        let harness = PostgresWindowHarness::fresh("a_chain_id_past_a_u32").await;

        harness
            .store()
            .append(WIDE_CHAIN, &header(500, b'a'), 0)
            .await
            .unwrap();

        assert_eq!(harness.rows_at(WIDE_CHAIN, 500).await, 1);
    }

    #[tokio::test]
    async fn round_trips_a_height_past_what_a_double_can_hold() {
        const HIGH: u64 = 9_007_199_254_740_993;

        let harness = PostgresWindowHarness::fresh("a_height_past_a_double").await;
        harness
            .store()
            .append(CHAIN_ID, &header(HIGH, b'a'), 0)
            .await
            .unwrap();

        let window = harness.store().load(CHAIN_ID).await.unwrap();

        assert_eq!(window.first().map(|held| held.number), Some(HIGH));
    }

    /// What makes the adapter's plain `to_string()` safe: the column insists on
    /// the lowercase hex `Display` gives, and the detector compares hashes for
    /// equality, so a differently-cased one reads as an unrecoverable reorg.
    #[tokio::test]
    async fn the_table_refuses_a_hash_that_is_not_lowercase() {
        let harness = PostgresWindowHarness::fresh("refuses_uppercase").await;
        let canonical = hash_of(b'a', 500).to_string();

        assert!(
            !harness
                .insert_raw(CHAIN_ID, 500, &canonical.to_uppercase())
                .await,
            "indexer_block_headers_hash_check let an uppercase hash in"
        );
        assert!(harness.insert_raw(CHAIN_ID, 501, &canonical).await);
    }
}

#[cfg(test)]
mod refusals {
    //! Which refusal each failure is reported as: the direction a database
    //! failure happened in, and the column that held what its type forbids.

    use postgres::{Limits, build_pool};

    use super::{PostgresBlockHeaderStore, retained};
    use crate::fixtures::{CHAIN_ID, hash_of, header};
    use crate::reorg::{BlockHeaderStore, Error};

    /// A port nothing listens on, so no server is needed and the pool's
    /// `create` deadline is never reached.
    const NOWHERE: &str = "postgres://postgres@127.0.0.1:1/nothing";

    /// All three carry the same cause, so swapping them is invisible to every
    /// other case and to the message an operator reads.
    #[tokio::test]
    async fn says_which_direction_a_database_failure_was_in() {
        let store = PostgresBlockHeaderStore::new(build_pool(NOWHERE, Limits::default()).unwrap());

        assert!(matches!(
            store.load(CHAIN_ID).await,
            Err(Error::WindowReadFailed(_))
        ));
        assert!(matches!(
            store.append(CHAIN_ID, &header(500, b'a'), 0).await,
            Err(Error::WindowWriteFailed(_))
        ));
        assert!(matches!(
            store.truncate(CHAIN_ID, None).await,
            Err(Error::WindowWriteFailed(_))
        ));
    }

    fn malformed(error: Error) -> (&'static str, String) {
        match error {
            Error::Malformed { column, value, .. } => (column, value),
            other => panic!("expected a malformed column: {other:?}"),
        }
    }

    #[test]
    fn refuses_a_negative_height() {
        let canonical = hash_of(b'a', 500).to_string();

        let refusal = retained(-1, &canonical, &canonical, 0).expect_err("a height below zero");

        assert_eq!(malformed(refusal), ("block_number", "-1".to_owned()));
    }

    #[test]
    fn names_which_of_the_two_hashes_was_malformed() {
        let canonical = hash_of(b'a', 500).to_string();

        assert_eq!(
            malformed(retained(500, "0xnope", &canonical, 0).unwrap_err()).0,
            "hash"
        );
        assert_eq!(
            malformed(retained(500, &canonical, "0xnope", 0).unwrap_err()).0,
            "parent_hash"
        );
    }

    /// The one column that substitutes rather than refusing.
    #[test]
    fn clamps_a_timestamp_before_the_epoch_rather_than_refusing() {
        let canonical = hash_of(b'a', 500).to_string();

        let header = retained(500, &canonical, &canonical, -1).expect("a clamp, not a refusal");

        assert_eq!(header.timestamp, 0);
    }
}
