//! A migrated schema, a store over it, and the rows put into it.
//!
//! **The write path is reproduced here rather than imported**: `CursorStore`,
//! which owns it, arrives with the loop in Phase 3. It is one `INSERT`, and the
//! column list is the migration's.

use alloy_primitives::B256;
use postgres::{Pool, build_pool, connection};
use refinery_core::{Migration, Runner};

use crate::cursor::PostgresSyncStatusStore;
use crate::cursor::conformance::{Advanced, Fixture};

pub(crate) struct PostgresFixture {
    pool: Pool,
    store: PostgresSyncStatusStore,
}

impl Fixture for PostgresFixture {
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

/// A schema per case, so cases can run at once.
async fn scratch(prefix: &str, case: &str) -> Pool {
    let base = std::env::var("POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned());
    let schema = format!("{prefix}_{case}");

    let admin = build_pool(&base).unwrap();
    connection(&admin)
        .await
        .unwrap()
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
        ))
        .await
        .unwrap();

    let separator = if base.contains('?') { '&' } else { '?' };
    let pool = build_pool(&format!(
        "{base}{separator}options=-c%20search_path%3D{schema}"
    ))
    .unwrap();

    let set: Vec<Migration> = migrations::POSTGRES
        .iter()
        .flat_map(|source| source.files)
        .map(|embedded| Migration::unapplied(embedded.label, embedded.sql).unwrap())
        .collect();

    let mut client = connection(&pool).await.unwrap();
    Runner::new(&set).run_async(&mut **client).await.unwrap();
    drop(client);

    pool
}

/// The spelling the table's `CHECK (last_hash ~ '^0x[0-9a-f]{64}$')` demands.
fn lower(hash: B256) -> String {
    format!("{hash:#x}")
}
