//! A migrated schema, a store over it, and the rows put into it.
//!
//! **`given_cursor` writes the row itself rather than going through
//! [`CursorStore`](crate::cursor::CursorStore)**, which now exists: that port
//! stamps `updated_at` with the server's `now()`, and these cases need a row
//! backdated by a stated number of seconds.

use postgres::{Pool, connection};

use super::{PostgresCursorStore, PostgresSyncStatusStore};
use crate::cursor::conformance::{Advanced, CursorHarness, Harness};
use crate::scratch::{lower, scratch};

pub(crate) struct PostgresHarness {
    pool: Pool,
    store: PostgresSyncStatusStore,
}

impl Harness for PostgresHarness {
    type Store = PostgresSyncStatusStore;

    async fn fresh(case: &str) -> Self {
        let pool = scratch("rust_indexing", case).await;

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
                        &i64::from(row.chain_id),
                        #[expect(
                            clippy::cast_possible_wrap,
                            reason = "a block height, which `bigint` is signed and blocks are not"
                        )]
                        &(row.last_block as i64),
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

pub(crate) struct PostgresCursorHarness {
    pool: Pool,
    store: PostgresCursorStore,
}

impl CursorHarness for PostgresCursorHarness {
    type Store = PostgresCursorStore;

    async fn fresh(case: &str) -> Self {
        let pool = scratch("rust_cursor", case).await;

        Self {
            store: PostgresCursorStore::new(pool.clone()),
            pool,
        }
    }

    fn store(&self) -> &Self::Store {
        &self.store
    }
}

impl PostgresCursorHarness {
    /// The same row, through the read port. Which is the point: one table, two
    /// adapters, and nothing else to keep them in step.
    pub(crate) fn as_read(&self) -> PostgresSyncStatusStore {
        PostgresSyncStatusStore::new(self.pool.clone())
    }

    pub(crate) async fn rows(&self) -> i64 {
        connection(&self.pool)
            .await
            .unwrap()
            .query_one("SELECT count(*) FROM indexer_cursor", &[])
            .await
            .unwrap()
            .get(0)
    }
}
