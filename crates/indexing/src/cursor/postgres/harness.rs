//! One migrated schema, both adapters over it, and the rows put in by hand.
//!
//! Both of this module's ports read and write the same row, so one harness
//! stands up both. `given_cursor` does not go through
//! [`CursorStore`](crate::cursor::CursorStore): that port stamps `updated_at`
//! with `now()`, and these cases need a row backdated by a stated interval.

use postgres::{Bigint, Pool, connection};
use test_db::Scratch;

use super::{PostgresCursorStore, PostgresSyncStatusStore};
use crate::cursor::conformance::{Advanced, CursorHarness, Harness};

pub(crate) struct PostgresHarness {
    pool: Pool,
    sync: PostgresSyncStatusStore,
    cursors: PostgresCursorStore,
}

impl PostgresHarness {
    /// The constructor both ports' `fresh` delegate to, so a case naming it
    /// directly does not have to say which trait it meant.
    pub(crate) async fn over(case: &str) -> Self {
        let pool = Scratch::migrated("rust_cursor", case).await.pool().clone();

        Self {
            sync: PostgresSyncStatusStore::new(pool.clone()),
            cursors: PostgresCursorStore::new(pool.clone()),
            pool,
        }
    }

    /// The same row through the read port, which is the point: one table, two
    /// adapters, and nothing else to keep them in step.
    pub(crate) fn sync(&self) -> &PostgresSyncStatusStore {
        &self.sync
    }

    pub(crate) fn cursors(&self) -> &PostgresCursorStore {
        &self.cursors
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

impl Harness for PostgresHarness {
    type Store = PostgresSyncStatusStore;

    async fn fresh(case: &str) -> Self {
        Self::over(case).await
    }

    fn store(&self) -> &Self::Store {
        self.sync()
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
                        &row.last_hash.to_string(),
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

impl CursorHarness for PostgresHarness {
    type Store = PostgresCursorStore;

    async fn fresh(case: &str) -> Self {
        Self::over(case).await
    }

    fn store(&self) -> &Self::Store {
        self.cursors()
    }
}
