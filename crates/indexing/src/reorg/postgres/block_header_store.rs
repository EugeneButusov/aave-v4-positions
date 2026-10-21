use alloy_primitives::B256;
use postgres::{Pool, TracedPool};
use tokio_postgres::Row;

use crate::chain::BlockHeader;
use crate::reorg::{BlockHeaderStore, Error};

/// No `ORDER BY`: the port promises none, the detector sorts what it reads, and
/// adding one here is a guarantee a later caller leans on without anything
/// having said it was made.
const LOAD: &str = "\
    SELECT block_number, hash, parent_hash, block_timestamp \
    FROM indexer_block_headers \
    WHERE chain_id = $1";

/// One statement, which is what "pruning rides along" asks for. A data-modifying
/// CTE gets the upsert and the prune in one round trip and one implicit
/// transaction, where a transaction around two statements would cost three on a
/// path that also runs once per header while a window refills.
///
/// **`AND chain_id` on the DELETE is load-bearing.** Without it one chain's
/// retention floor wipes every other chain's window, and no single-chain case
/// can see the difference.
///
/// The DELETE reads the pre-INSERT snapshot, so the row just written is never a
/// prune candidate. Harmless, because `retain_from <= header.number` is the
/// port's stated precondition.
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
    async fn load(&self, chain_id: u32) -> Result<Vec<BlockHeader>, Error> {
        self.db
            .query(
                "SELECT indexer_block_headers",
                LOAD,
                &[&i64::from(chain_id)],
            )
            .await?
            .iter()
            .map(header)
            .collect()
    }

    async fn append(
        &self,
        chain_id: u32,
        header: &BlockHeader,
        retain_from: u64,
    ) -> Result<(), Error> {
        self.db
            .execute(
                "INSERT indexer_block_headers",
                APPEND,
                &[
                    &i64::from(chain_id),
                    &signed(header.number),
                    &lower(header.hash),
                    &lower(header.parent_hash),
                    &signed(header.timestamp),
                    &signed(retain_from),
                ],
            )
            .await?;

        Ok(())
    }

    async fn truncate(&self, chain_id: u32, last_valid_block: Option<u64>) -> Result<(), Error> {
        // A separate statement rather than passing -1, because the column is
        // unsigned in every sense that matters and a sentinel that only works
        // while `bigint` is signed is a sentinel waiting to stop working.
        match last_valid_block {
            Some(block) => {
                self.db
                    .execute(
                        "DELETE indexer_block_headers",
                        TRUNCATE,
                        &[&i64::from(chain_id), &signed(block)],
                    )
                    .await?;
            }
            None => {
                self.db
                    .execute(
                        "DELETE indexer_block_headers",
                        CLEAR,
                        &[&i64::from(chain_id)],
                    )
                    .await?;
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

/// The four columns as the driver hands them over.
///
/// Separate from the row so every refusal below is reachable: the table's own
/// `CHECK`s put all of them out of reach through it, which would leave the
/// branch that decides what a malformed window does untested.
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
        // Clamped rather than refused, and the only one of these that is: a
        // timestamp before the epoch is a chain nobody indexes, and nothing
        // here compares one against a clock. A hash or a height is the other
        // way round — no answer beats a plausible one.
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

#[expect(
    clippy::cast_possible_wrap,
    reason = "a block height or a unix second, neither of which reaches 2^63"
)]
fn signed(value: u64) -> i64 {
    value as i64
}

/// The spelling the table's two `CHECK (… ~ '^0x[0-9a-f]{64}$')` demand.
fn lower(hash: B256) -> String {
    format!("{hash:#x}")
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

    use crate::fixtures::{CHAIN_ID, hash_of, header};
    use crate::reorg::BlockHeaderStore;
    use crate::reorg::conformance::Harness as _;
    use crate::reorg::postgres::harness::PostgresWindowHarness;
    use crate::scratch::lower;

    /// Asserted against the table, because `load` would hand two rows back as a
    /// window of two and the ancestor walk would read one of them as canonical.
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

    /// `bigint` holds it; nothing reaches this height today. The point is that
    /// the column's width is honoured rather than narrowed on the way through.
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

    /// Why the adapter lowercases rather than trusting its caller: the detector
    /// compares hashes for equality, so a differently-cased one would match
    /// nothing and read as a permanent, unrecoverable reorg.
    #[tokio::test]
    async fn the_table_refuses_a_hash_that_is_not_lowercase() {
        let harness = PostgresWindowHarness::fresh("refuses_uppercase").await;
        let canonical = lower(hash_of(b'a', 500));

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
    //! What a window read does with a column holding what its type says it
    //! cannot. Unreachable through the table, and decided here all the same.

    use super::retained;
    use crate::fixtures::hash_of;
    use crate::reorg::Error;
    use crate::scratch::lower;

    fn malformed(error: Error) -> (&'static str, String) {
        match error {
            Error::Malformed { column, value, .. } => (column, value),
            other => panic!("expected a malformed column: {other:?}"),
        }
    }

    #[test]
    fn refuses_a_negative_height() {
        let canonical = lower(hash_of(b'a', 500));

        let refusal = retained(-1, &canonical, &canonical, 0).expect_err("a height below zero");

        assert_eq!(malformed(refusal), ("block_number", "-1".to_owned()));
    }

    #[test]
    fn names_which_of_the_two_hashes_was_malformed() {
        let canonical = lower(hash_of(b'a', 500));

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
        let canonical = lower(hash_of(b'a', 500));

        let header = retained(500, &canonical, &canonical, -1).expect("a clamp, not a refusal");

        assert_eq!(header.timestamp, 0);
    }
}
