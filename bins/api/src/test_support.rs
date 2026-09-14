//! One `App` from the environment's defaults, so a case says what it is testing.
//!
//! [crates.io's `TestApp`](https://github.com/rust-lang/crates.io/blob/main/src/tests/util/test_app.rs)
//! at this service's scale: the cases below drive the real router rather than
//! reassembling one, so a wiring mistake fails them rather than hiding behind a
//! parallel definition.
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
use ops::{ShutdownFlag, Uptime};
use postgres::Pool;
use prices::{Error as PriceError, ReserveKey, ReservePrice, ReservePriceStore};
use time::OffsetDateTime;
use token_metadata::{Error as LabelError, TokenLabel, TokenMetadataStore};
use tower::ServiceExt;

use crate::app::{self as app, App};
use crate::config::Staleness;
use crate::errors::BoxedAppError;
use crate::positions::Cursors;

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
            positions: Arc::new(Positions(self.page)),
            tokens: Arc::new(Labels(self.labels)),
            prices: Arc::new(Prices {
                latest: self.prices,
                reads: Arc::clone(&self.price_reads),
            }),
            sync: Arc::new(Synced(self.sync)),
            cursors: Cursors::new(SECRET).expect("HMAC takes a key of any length"),
            staleness: self.staleness,
            prefix: "api".to_owned(),
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
