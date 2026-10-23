//! A migrated schema, a store over it, and the rows put into it.
//!
//! **The write path is reproduced here rather than imported**, for the reason
//! `aave-positions`'s harness gives about the ledgers: the enrichment processor
//! that owns `put` belongs to a crate that does not exist yet. It is one
//! `INSERT`, and the column list is the migration's.

use alloy_primitives::Address;
use postgres::{Bigint, Pool, connection};
use test_db::Scratch;

use super::PostgresTokenMetadataStore;
use crate::conformance::{Harness, Stored};

pub(crate) struct PostgresHarness {
    pool: Pool,
    store: PostgresTokenMetadataStore,
}

impl Harness for PostgresHarness {
    type Store = PostgresTokenMetadataStore;

    /// A schema per case, not per file. `cargo` runs these concurrently and
    /// they assert on the rows that exist, so a shared one has them creating
    /// and dropping each other's fixtures.
    async fn fresh(case: &str) -> Self {
        let pool = Scratch::migrated("rust_token_metadata", case)
            .await
            .pool()
            .clone();

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
                        &Bigint(row.chain_id.into()),
                        // The column's `CHECK` allows nothing else, and this is
                        // what makes the checksummed-lookup case mean anything.
                        &lower(row.token),
                        &row.symbol,
                        &row.name,
                        &18i16,
                        &Bigint(1),
                    ],
                )
                .await
                .unwrap();
        }
    }
}

fn lower(address: Address) -> String {
    format!("{address:#x}")
}
