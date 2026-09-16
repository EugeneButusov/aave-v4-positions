//! One `App` from the environment's defaults, so a case says what it is testing.
//!
//! The cases below drive the real router rather than reassembling one, so a
//! wiring mistake fails them rather than hiding behind a parallel definition.
//!
//! **The four read ports are doubles and no database is reached.** What a store
//! does with a query is its own crate's conformance suite to prove against a
//! real server; what this binary adds is the join, the wire shape and the
//! refusals. The two connections stay real for the readiness probe, which pings
//! them.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aave_positions::store::{Error as StoreError, PositionPage, PositionQuery, PositionStore};
use alloy_primitives::{Address, B256};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use clickhouse_client::clickhouse::Client;
use indexing::{Error as SyncError, SyncStatus, SyncStatusStore};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use ops::{ShutdownFlag, Uptime};
use postgres::Pool;
use prices::{Error as PriceError, ReserveKey, ReservePrice, ReservePriceStore};
use time::OffsetDateTime;
use token_metadata::{Error as LabelError, TokenLabel, TokenMetadataStore};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt as _;

use crate::app::{self as app, App};
use crate::config::Staleness;
use crate::errors::BoxedAppError;
use crate::positions::Signer;

/// Thirty-two bytes, which is the minimum `config` enforces.
pub(crate) const SECRET: &str = "a-test-key-that-is-long-enough!!";

/// The two the `config` defaults name, so a case that does not care about
/// staleness reads the thresholds a deployment gets.
const STALENESS: Staleness = Staleness {
    sync: 60,
    price: 300,
};

/// The same defaults every other suite in this workspace reads.
pub(crate) fn clickhouse() -> Client {
    clickhouse_client::build_client(clickhouse_client::Config {
        url: std::env::var("CLICKHOUSE_URL").unwrap_or_else(|_| "http://localhost:8123".to_owned()),
        database: "default".to_owned(),
        user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
        password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
    })
}

pub(crate) fn postgres() -> Pool {
    let url = std::env::var("POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned());

    ::postgres::build_pool(&url).unwrap()
}

/// Port 1 is reserved and nothing listens on it, so the pool's first dial is
/// refused — a dependency that is down rather than one that is slow.
pub(crate) fn unreachable_postgres() -> Pool {
    ::postgres::build_pool("postgres://postgres@127.0.0.1:1/postgres").unwrap()
}

pub(crate) fn state(postgres: Pool) -> App {
    Stores::default().app(postgres)
}

/// The router this binary actually serves, over the given Postgres.
pub(crate) fn handler(postgres: Pool) -> Router {
    app::handler(state(postgres))
}

/// What the four ports answer, before a case changes its mind about any of them.
/// Plain fields rather than a builder: a `with_` method per field would be six
/// methods that each assign one field.
pub(crate) struct Stores {
    /// `None` is a chain this deployment has never indexed.
    pub(crate) sync: Option<SyncStatus>,
    pub(crate) page: PositionPage,
    pub(crate) labels: HashMap<Address, TokenLabel>,
    pub(crate) prices: HashMap<ReserveKey, ReservePrice>,
    pub(crate) staleness: Staleness,

    /// How many times the price store was asked. The point of it is the answer
    /// `0`: an explicit `as_of` skips the read rather than discarding it.
    pub(crate) price_reads: Arc<AtomicUsize>,
}

impl Default for Stores {
    fn default() -> Self {
        Self {
            sync: Some(synced()),
            page: PositionPage {
                items: Vec::new(),
                valued_at: VALUED_AT,
                next: None,
            },
            labels: HashMap::new(),
            prices: HashMap::new(),
            staleness: STALENESS,
            price_reads: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// A fixed instant, so a body a case asserts whole does not move with the clock.
/// 2026-07-25T17:20:00Z.
pub(crate) const VALUED_AT: u64 = 1_785_000_000;

pub(crate) fn synced() -> SyncStatus {
    SyncStatus {
        chain_id: 1,
        last_block: 25_652_535,
        last_hash: format!("0x{}", "ab".repeat(32))
            .parse::<B256>()
            .expect("a literal hash"),
        updated_at: OffsetDateTime::from_unix_timestamp(1_785_000_071)
            .expect("a representable instant"),
        age_seconds: 7,
    }
}

impl Stores {
    pub(crate) fn app(self, postgres: Pool) -> App {
        App {
            uptime: Uptime::now(),
            shutdown: ShutdownFlag::new(),
            clickhouse: clickhouse(),
            postgres,
            positions: Box::new(Positions(self.page)),
            tokens: Box::new(Labels(self.labels)),
            prices: Box::new(Prices {
                latest: self.prices,
                reads: Arc::clone(&self.price_reads),
            }),
            sync: Box::new(Synced(self.sync)),
            signer: Signer::new(SECRET).expect("HMAC takes a key of any length"),
            staleness: self.staleness,
            prefix: "api".to_owned(),
            docs_path: "docs".to_owned(),
            docs_assets: "/usr/share/api/docs".to_owned(),
        }
    }

    /// The real router, with these four in front of it.
    pub(crate) fn handler(self) -> Router {
        app::handler(self.app(postgres()))
    }
}

struct Positions(PositionPage);

#[async_trait::async_trait]
impl PositionStore for Positions {
    async fn list(&self, _query: &PositionQuery) -> Result<PositionPage, StoreError> {
        Ok(self.0.clone())
    }
}

struct Labels(HashMap<Address, TokenLabel>);

#[async_trait::async_trait]
impl TokenMetadataStore for Labels {
    async fn labels(&self, _chain_id: u32) -> Result<HashMap<Address, TokenLabel>, LabelError> {
        Ok(self.0.clone())
    }
}

struct Prices {
    latest: HashMap<ReserveKey, ReservePrice>,
    reads: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl ReservePriceStore for Prices {
    async fn latest(
        &self,
        _chain_id: u32,
    ) -> Result<HashMap<ReserveKey, ReservePrice>, PriceError> {
        self.reads.fetch_add(1, Ordering::Relaxed);

        Ok(self.latest.clone())
    }
}

struct Synced(Option<SyncStatus>);

#[async_trait::async_trait]
impl SyncStatusStore for Synced {
    async fn get(&self, _chain_id: u32) -> Result<Option<SyncStatus>, SyncError> {
        Ok(self.0.clone())
    }
}

/// Status, body and the echoed request id — the three things every case reads.
pub(crate) async fn get(
    router: Router,
    request: Request<Body>,
) -> (StatusCode, String, Option<String>) {
    let response = router.oneshot(request).await.unwrap();

    let status = response.status();
    let request_id = response
        .headers()
        .get("x-request-id")
        .map(|value| value.to_str().unwrap().to_owned());
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    (
        status,
        String::from_utf8(body.to_vec()).unwrap(),
        request_id,
    )
}

/// Status and body of a failure, which is the pair an error case reads.
pub(crate) async fn answered(error: BoxedAppError) -> (StatusCode, String) {
    let response = error.into_response();

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    (status, String::from_utf8(body.to_vec()).unwrap())
}

/// Everything one request produced, through a subscriber of the case's own.
///
/// **Nothing process-wide.** Spans, log lines and the echoed header all come
/// from one `with_default`, so cases can run in parallel without reading each
/// other's telemetry — which a global provider would make them do.
pub(crate) struct Observed {
    pub(crate) request_id: Option<String>,
    pub(crate) spans: Vec<SpanData>,

    /// The JSON lines the formatter wrote, as a deployment would read them.
    pub(crate) logged: String,
}

pub(crate) fn observed(router: Router, request: Request<Body>) -> Observed {
    // The propagator is a global by design: it is what the ClickHouse driver
    // reads to put `traceparent` on its own request. Every case setting the same
    // one is not a case setting a different one.
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let written = Written::default();
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(written.clone()),
        )
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer(telemetry::SCOPE)));

    let request_id = tracing::subscriber::with_default(subscriber, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                router
                    .oneshot(request)
                    .await
                    .unwrap()
                    .headers()
                    .get("x-request-id")
                    .map(|value| value.to_str().unwrap().to_owned())
            })
    });
    provider.force_flush().unwrap();

    Observed {
        request_id,
        spans: exporter.get_finished_spans().unwrap(),
        logged: written.read(),
    }
}

/// What the formatter wrote, held where a case can read it.
#[derive(Clone, Default)]
pub(crate) struct Written(Arc<std::sync::Mutex<Vec<u8>>>);

impl Written {
    fn read(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for Written {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Written {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
