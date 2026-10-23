//! A migrated schema, a store over it, and the rows put into it.
//!
//! **The write path is reproduced here rather than imported**: the refresher
//! that owns `put` belongs to Phase 4. It is one `INSERT`, and the column list
//! is the migration's.

use alloy_primitives::Address;
use postgres::{Bigint, Pool, connection};
use test_db::Scratch;

use super::PostgresReservePriceStore;
use crate::conformance::{Harness, Quoted};

pub(crate) struct PostgresHarness {
    pool: Pool,
    store: PostgresReservePriceStore,
}

impl Harness for PostgresHarness {
    type Store = PostgresReservePriceStore;

    async fn fresh(case: &str) -> Self {
        let pool = Scratch::migrated("rust_prices", case).await.pool().clone();

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
                        &Bigint(row.chain_id.into()),
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

/// The spelling the table's `CHECK (spoke ~ '^0x[0-9a-f]{40}$')` demands.
fn lower(address: Address) -> String {
    format!("{address:#x}")
}
