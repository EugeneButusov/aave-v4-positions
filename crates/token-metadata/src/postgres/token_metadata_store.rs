//! Labels in Postgres.

use std::collections::HashMap;

use alloy_primitives::Address;
use postgres::{Pool, connection};
use tokio_postgres::Row;
use tracing::Instrument as _;

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
        let rows = client
            .query(LABELS, &[&i64::from(chain_id)])
            .instrument(postgres::query_span("SELECT", "token_metadata", LABELS))
            .await?;

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

    use std::sync::{Arc, Mutex};

    use tracing::instrument::WithSubscriber as _;
    use tracing_subscriber::fmt::format::FmtSpan;

    use crate::TokenMetadataStore;
    use crate::conformance::{self, Harness};
    use crate::postgres::harness::PostgresHarness;

    macro_rules! conformance {
        ($($case:ident),* $(,)?) => {
            $(
                #[tokio::test]
                async fn $case() {
                    conformance::$case::<PostgresHarness>().await;
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

    /// Somewhere a case can read a span back from. Ten lines because
    /// `MakeWriter` is implemented for any `Fn() -> impl Write`, so this needs
    /// no impl of its own.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Sink {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// The store's own read, seen as a trace sees it.
    ///
    /// **`tokio-postgres` opens no span** — it speaks `log`, not `tracing` — so
    /// without the `instrument` on the read above, a trace through this store is
    /// a gap between the request and the answer.
    #[tokio::test]
    async fn the_read_is_one_span_a_trace_can_see() {
        let harness = PostgresHarness::fresh("traces_its_read").await;
        let sink = Sink::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer({
                let sink = sink.clone();
                move || sink.clone()
            })
            .with_ansi(false)
            .with_span_events(FmtSpan::CLOSE)
            .finish();

        harness
            .store()
            .labels(1)
            .with_subscriber(subscriber)
            .await
            .unwrap();

        let written = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();

        assert!(
            written.contains(r#"otel.name="SELECT token_metadata""#),
            "{written}"
        );
        assert!(
            written.contains(r#"db.system.name="postgresql""#),
            "{written}"
        );
    }
}
