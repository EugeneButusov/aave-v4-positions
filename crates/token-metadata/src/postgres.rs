//! Labels in Postgres.

use std::collections::HashMap;

use alloy_primitives::Address;
use postgres::{Pool, connection};
use tokio_postgres::Row;

use crate::{Error, TokenLabel, TokenMetadataStore};

/// Keyed by chain and nothing else, so it does not depend on the page and can
/// be issued beside the ClickHouse query rather than after it.
const LABELS: &str = "\
    SELECT token, symbol, name \
    FROM token_metadata \
    WHERE chain_id = $1";

pub struct PostgresTokenMetadataStore {
    pool: Pool,
}

impl PostgresTokenMetadataStore {
    /// Takes the pool rather than making one, for the reason
    /// `crates/postgres` gives: a store that reaches for the environment cannot
    /// be used twice in one process against two servers.
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl TokenMetadataStore for PostgresTokenMetadataStore {
    async fn labels(&self, chain_id: u32) -> Result<HashMap<Address, TokenLabel>, Error> {
        let client = connection(&self.pool).await?;
        // `bigint`, so the parameter is signed however the port spells it.
        let rows = client.query(LABELS, &[&i64::from(chain_id)]).await?;

        rows.iter().map(label).collect()
    }
}

fn label(row: &Row) -> Result<(Address, TokenLabel), Error> {
    let token: &str = row.get("token");

    let token = token.parse().map_err(|_| Error::Malformed {
        column: "token",
        expected: "20-byte address",
        value: token.to_owned(),
    })?;

    Ok((
        token,
        TokenLabel {
            symbol: row.get("symbol"),
            name: row.get("name"),
        },
    ))
}

#[cfg(test)]
mod tests {
    //! The port's specification, against a real Postgres.

    use crate::conformance;
    use crate::harness::PostgresFixture;

    macro_rules! conformance {
        ($($case:ident),* $(,)?) => {
            $(
                #[tokio::test]
                async fn $case() {
                    conformance::$case::<PostgresFixture>().await;
                }
            )*
        };
    }

    conformance! {
        reads_every_label_on_the_chain,
        leaves_out_a_chain_it_was_not_asked_about,
        keeps_a_token_that_answered_with_no_symbol,
        omits_a_token_with_no_row,
        answers_a_checksummed_address_from_a_lower_cased_row,
        is_empty_for_a_chain_nobody_has_enriched,
    }
}
