//! A migrated schema, a store over it, and the rows put into it.
//!
//! **The write path is reproduced here rather than imported**: the refresher
//! that owns `put` belongs to Phase 4. It is one `INSERT`, and the column list
//! is the migration's.

use alloy_primitives::Address;
use postgres::{Pool, build_pool, connection};
use refinery_core::{Migration, Runner};

use crate::PostgresReservePriceStore;
use crate::contract::{Fixture, Quoted};

pub(crate) struct Postgres {
    pool: Pool,
    store: PostgresReservePriceStore,
}

impl Fixture for Postgres {
    type Store = PostgresReservePriceStore;

    async fn fresh(case: &str) -> Self {
        let pool = scratch("rust_prices", case).await;

        Self {
            store: PostgresReservePriceStore::new(pool.clone()),
            pool,
        }
    }

    fn store(&self) -> &Self::Store {
        &self.store
    }

    async fn given_prices(&self, rows: &[Quoted]) {
        let client = connection(&self.pool).await.unwrap();

        for row in rows {
            client
                .execute(
                    // `priced_at` is written relative to the server's own
                    // clock, never this process's — the same rule the read
                    // computes `age_seconds` under.
                    "INSERT INTO reserve_prices \
                     (chain_id, spoke, reserve_id, price, priced_at) \
                     VALUES ($1, $2, $3::text::numeric, $4::text::numeric, \
                             now() - make_interval(secs => $5::double precision))",
                    &[
                        &i64::from(row.chain_id),
                        &lower(row.spoke),
                        &row.reserve_id.to_string(),
                        &row.price,
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

/// A schema per case, so cases can run at once. Shared with the other stores'
/// harnesses in shape but not in code — each names its own prefix, and a crate
/// that exported this would be a test-support crate nothing else wants yet.
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

/// The spelling the table's `CHECK (spoke ~ '^0x[0-9a-f]{40}$')` demands.
fn lower(address: Address) -> String {
    format!("{address:#x}")
}
