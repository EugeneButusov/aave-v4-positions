//! A migrated schema and a window over it.
//!
//! No `given_*`: this port writes, so the suite puts its own state in front of
//! it through [`BlockHeaderStore::append`](crate::reorg::BlockHeaderStore).

use postgres::{Bigint, Pool, connection};
use test_db::Scratch;
use units::ChainId;

use super::PostgresBlockHeaderStore;
use crate::reorg::conformance::Harness;

pub(crate) struct PostgresWindowHarness {
    pool: Pool,
    store: PostgresBlockHeaderStore,
}

impl Harness for PostgresWindowHarness {
    type Store = PostgresBlockHeaderStore;

    async fn fresh(case: &str) -> Self {
        let pool = Scratch::migrated("rust_window", case).await.pool().clone();

        Self {
            store: PostgresBlockHeaderStore::new(pool.clone()),
            pool,
        }
    }

    fn store(&self) -> &Self::Store {
        &self.store
    }
}

impl PostgresWindowHarness {
    /// For the cases that assert against the table rather than through the
    /// port — which is the only way to see a second row that `load` would
    /// quietly hand back as one.
    pub(crate) async fn rows_at(&self, chain_id: ChainId, block: u64) -> i64 {
        connection(&self.pool)
            .await
            .unwrap()
            .query_one(
                "SELECT count(*) FROM indexer_block_headers \
                 WHERE chain_id = $1 AND block_number = $2",
                &[&Bigint(chain_id.into()), &Bigint(block)],
            )
            .await
            .unwrap()
            .get(0)
    }

    /// Writes a row the adapter would never write, so the table's own
    /// constraint is what answers.
    pub(crate) async fn insert_raw(&self, chain_id: ChainId, block: u64, hash: &str) -> bool {
        connection(&self.pool)
            .await
            .unwrap()
            .execute(
                "INSERT INTO indexer_block_headers \
                 (chain_id, block_number, hash, parent_hash, block_timestamp) \
                 VALUES ($1, $2, $3, $3, 0)",
                &[&Bigint(chain_id.into()), &Bigint(block), &hash],
            )
            .await
            .is_ok()
    }
}
