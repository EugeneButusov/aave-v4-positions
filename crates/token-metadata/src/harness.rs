//! A migrated schema, a store over it, and the rows put into it.
//!
//! **The write path is reproduced here rather than imported**, for the reason
//! `aave-positions`'s harness gives about the ledgers: the enrichment processor
//! that owns `put` belongs to a crate that does not exist yet. It is one
//! `INSERT`, and the column list is the migration's.

use alloy_primitives::Address;
use postgres::{Pool, build_pool, connection};
use refinery_core::{Migration, Runner};

use crate::PostgresTokenMetadataStore;
use crate::conformance::{Fixture, Stored};

pub(crate) struct PostgresFixture {
    pool: Pool,
    store: PostgresTokenMetadataStore,
}

impl Fixture for PostgresFixture {
    type Store = PostgresTokenMetadataStore;

    /// A schema per case, not per file. `cargo` runs these concurrently and
    /// they assert on the rows that exist, so a shared one has them creating
    /// and dropping each other's fixtures.
    async fn fresh(case: &str) -> Self {
        let base = std::env::var("POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned());
        let schema = format!("rust_token_metadata_{case}");

        let admin = build_pool(&base).unwrap();
        connection(&admin)
            .await
            .unwrap()
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
            ))
            .await
            .unwrap();

        // `options` is libpq's, and tokio-postgres parses it out of the URL —
        // which is what lets a schema be chosen without `crates/postgres`
        // growing a parameter only a test would pass.
        let separator = if base.contains('?') { '&' } else { '?' };
        let pool = build_pool(&format!(
            "{base}{separator}options=-c%20search_path%3D{schema}"
        ))
        .unwrap();

        migrate(&pool).await;

        Self {
            store: PostgresTokenMetadataStore::new(pool.clone()),
            pool,
        }
    }

    fn store(&self) -> &Self::Store {
        &self.store
    }

    async fn given_labels(&self, rows: &[Stored]) {
        let client = connection(&self.pool).await.unwrap();

        for row in rows {
            client
                .execute(
                    "INSERT INTO token_metadata \
                     (chain_id, token, symbol, name, token_decimals, fetched_at_block) \
                     VALUES ($1, $2, $3, $4, $5, $6)",
                    &[
                        &i64::from(row.chain_id),
                        // The column's `CHECK` allows nothing else, and this is
                        // what makes the checksummed-lookup case mean anything.
                        &lower(row.token),
                        &row.symbol,
                        &row.name,
                        &18i16,
                        &1i64,
                    ],
                )
                .await
                .unwrap();
        }
    }
}

/// The schema `bins/migrate` applies, so a case's database is the deployment's
/// rather than a second reading of the same directories.
async fn migrate(pool: &Pool) {
    let set: Vec<Migration> = migrations::POSTGRES
        .iter()
        .flat_map(|source| source.files)
        .map(|embedded| Migration::unapplied(embedded.label, embedded.sql).unwrap())
        .collect();

    let mut client = connection(pool).await.unwrap();
    Runner::new(&set).run_async(&mut **client).await.unwrap();
}

fn lower(address: Address) -> String {
    format!("{address:#x}")
}
