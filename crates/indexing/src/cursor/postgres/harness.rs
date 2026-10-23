//! A migrated schema, a store over it, and the rows put into it.
//!
//! **The write path is reproduced here rather than imported**: `CursorStore`,
//! which owns it, arrives with the loop in Phase 3. It is one `INSERT`, and the
//! column list is the migration's.

use alloy_primitives::B256;
use postgres::{Bigint, Pool, connection};
use test_db::Scratch;

use super::PostgresSyncStatusStore;
use crate::cursor::conformance::{Advanced, Harness};

pub(crate) struct PostgresHarness {
    pool: Pool,
    store: PostgresSyncStatusStore,
}

impl Harness for PostgresHarness {
    type Store = PostgresSyncStatusStore;

    async fn fresh(case: &str) -> Self {
        let pool = Scratch::migrated("rust_indexing", case)
            .await
            .pool()
            .clone();

        Self {
            store: PostgresSyncStatusStore::new(pool.clone()),
            pool,
        }
    }

    fn store(&self) -> &Self::Store {
        &self.store
    }

    async fn given_cursor(&self, rows: &[Advanced]) {
        let client = connection(&self.pool).await.unwrap();

        for row in rows {
            client
                .execute(
                    // `updated_at` relative to the server's own clock, never
                    // this process's — the same rule the read computes
                    // `age_seconds` under.
                    "INSERT INTO indexer_cursor \
                     (chain_id, last_block, last_hash, updated_at) \
                     VALUES ($1, $2, $3, \
                             now() - make_interval(secs => $4::double precision))",
                    &[
                        &Bigint(row.chain_id.into()),
                        &Bigint(row.last_block),
                        &lower(row.last_hash),
                        #[expect(
                            clippy::cast_precision_loss,
                            reason = "a test's staleness window, in whole seconds under a minute"
                        )]
                        &(row.aged_seconds as f64),
                    ],
                )
                .await
                .unwrap();
        }
    }
}

/// The spelling the table's `CHECK (last_hash ~ '^0x[0-9a-f]{64}$')` demands.
fn lower(hash: B256) -> String {
    format!("{hash:#x}")
}
