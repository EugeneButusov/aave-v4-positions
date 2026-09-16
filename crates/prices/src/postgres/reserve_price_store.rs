//! Prices in Postgres.

use std::collections::HashMap;

use alloy_primitives::U256;
use postgres::{Pool, connection};
use time::OffsetDateTime;
use tokio_postgres::Row;
use tracing::Instrument as _;

use crate::{Error, ReserveKey, ReservePrice, ReservePriceStore};

/// Keyed by chain and nothing else, so it does not depend on the page and can
/// be issued beside the ClickHouse query rather than after it.
///
/// **`age_seconds` is computed here, not by the reader**, for the reason
/// `indexer_cursor` gives: `priced_at` is this server's own `now()`, so a
/// reader subtracting it from its own clock reports skew as staleness — and a
/// fast reader would mark a fresh price stale, which flags every USD value on
/// the page as suspect. Floored to whole seconds: six decimal places on a
/// figure compared against a threshold in minutes is precision nobody can use.
///
/// **The two wide columns come back as text.** `numeric(78,0)` holds values a
/// `f64` has already rounded the tail off, and the driver has no lossless
/// integer of that width — the same reason the ClickHouse store keeps its
/// `toString(...)` casts (§7.5).
const LATEST: &str = "\
    SELECT \
        spoke, \
        reserve_id::text AS reserve_id, \
        price::text AS price, \
        priced_at, \
        floor(EXTRACT(EPOCH FROM (now() - priced_at)))::bigint AS age_seconds \
    FROM reserve_prices \
    WHERE chain_id = $1";

pub struct PostgresReservePriceStore {
    pool: Pool,
}

impl PostgresReservePriceStore {
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ReservePriceStore for PostgresReservePriceStore {
    async fn latest(&self, chain_id: u32) -> Result<HashMap<ReserveKey, ReservePrice>, Error> {
        let client = connection(&self.pool).await?;
        let rows = client
            .query(LATEST, &[&i64::from(chain_id)])
            .instrument(postgres::query_span("SELECT", "reserve_prices", LATEST))
            .await?;

        rows.iter().map(priced).collect()
    }
}

fn priced(row: &Row) -> Result<(ReserveKey, ReservePrice), Error> {
    let spoke: &str = row.get("spoke");
    let reserve_id: &str = row.get("reserve_id");
    let price: &str = row.get("price");
    let priced_at: OffsetDateTime = row.get("priced_at");
    let age_seconds: i64 = row.get("age_seconds");

    let key = ReserveKey {
        spoke: spoke.parse().map_err(|_| Error::Malformed {
            column: "spoke",
            expected: "20-byte address",
            value: spoke.to_owned(),
        })?,
        reserve_id: unsigned("reserve_id", reserve_id)?,
    };

    Ok((
        key,
        ReservePrice {
            price: unsigned("price", price)?,
            priced_at,
            // Clamped rather than refused: negative means the row was
            // written by a clock ahead of the one reading it, and "not stale"
            // is the honest answer to that.
            age_seconds: age_seconds.try_into().unwrap_or(0),
        },
    ))
}

fn unsigned(column: &'static str, value: &str) -> Result<U256, Error> {
    U256::from_str_radix(value, 10).map_err(|_| Error::Malformed {
        column,
        expected: "uint256",
        value: value.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    //! The port's specification, against a real Postgres.

    use std::sync::{Arc, Mutex};

    use tracing::instrument::WithSubscriber as _;
    use tracing_subscriber::fmt::format::FmtSpan;

    use crate::ReservePriceStore;
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
        reads_every_price_on_the_chain,
        leaves_out_a_chain_it_was_not_asked_about,
        keeps_two_spokes_pricing_the_same_reserve_id_apart,
        omits_a_reserve_nobody_has_priced,
        answers_a_checksummed_spoke_from_a_lower_cased_row,
        reads_a_price_past_what_a_double_can_hold,
        ages_a_price_by_the_database_clock,
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
            .latest(1)
            .with_subscriber(subscriber)
            .await
            .unwrap();

        let written = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();

        assert!(
            written.contains(r#"otel.name="SELECT reserve_prices""#),
            "{written}"
        );
        assert!(
            written.contains(r#"db.system.name="postgresql""#),
            "{written}"
        );
    }
}
