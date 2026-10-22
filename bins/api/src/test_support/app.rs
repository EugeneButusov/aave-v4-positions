//! One `App` from the environment's defaults, so a case says what it is testing.
//!
//! The cases drive the real router rather than reassembling one, so a wiring
//! mistake fails them rather than hiding behind a parallel definition.
//!
//! The four read ports are doubles: what a store does with a query is its own
//! crate's conformance suite to prove, and what this binary adds is the join,
//! the wire shape and the refusals. The two connections stay real for the
//! readiness probe.

use std::collections::HashMap;
use std::sync::Arc;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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
use units::ChainId;

use crate::app::{self as app, App};
use crate::config::Staleness;
use crate::errors::ApiError;
use crate::positions::Signer;

/// Thirty-two bytes, which is the minimum `config` enforces.
const SECRET: &str = "a-test-key-that-is-long-enough!!";

/// The two the `config` defaults name, so a case that does not care about
/// staleness reads the thresholds a deployment gets.
const STALENESS: Staleness = Staleness {
    sync: 60,
    price: 300,
};

/// The same variables `config` reads, so the readiness cases ask the database a
/// deployment would have migrated rather than one named here.
fn config() -> clickhouse_client::Config {
    clickhouse_client::Config {
        url: std::env::var("CLICKHOUSE_URL").unwrap_or_else(|_| "http://localhost:8123".to_owned()),
        database: std::env::var("CLICKHOUSE_DATABASE").unwrap_or_else(|_| "default".to_owned()),
        user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
        password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
        max_execution: Duration::from_secs(60),
    }
}

pub(crate) fn clickhouse() -> Client {
    clickhouse_client::build_client(config())
}

/// A database of this server's that no migration has reached.
///
/// Created rather than named, because the readiness cases run wherever the
/// suite does and the one database the environment names has been migrated.
pub(crate) async fn unmigrated_clickhouse() -> Client {
    let database = "rust_api_unmigrated";
    let admin = clickhouse();
    for statement in [
        format!("DROP DATABASE IF EXISTS {database} SYNC"),
        format!("CREATE DATABASE {database}"),
    ] {
        admin.query(&statement).execute().await.unwrap();
    }

    clickhouse_client::build_client(clickhouse_client::Config {
        database: database.to_owned(),
        ..config()
    })
}

pub(crate) fn postgres_url() -> String {
    std::env::var("POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned())
}

pub(crate) fn postgres() -> Pool {
    ::postgres::build_pool(&postgres_url(), ::postgres::Limits::default()).unwrap()
}

/// Port 1 is reserved and nothing listens on it, so the pool's first dial is
/// refused — a dependency that is down rather than one that is slow.
pub(crate) fn unreachable_postgres() -> Pool {
    ::postgres::build_pool(
        "postgres://postgres@127.0.0.1:1/postgres",
        ::postgres::Limits::default(),
    )
    .unwrap()
}

pub(crate) fn state(postgres: Pool) -> App {
    Stores::default().app(postgres)
}

pub(crate) fn handler(postgres: Pool) -> Router {
    app::handler(state(postgres))
}

/// What the four ports answer, before a case changes its mind. Plain fields
/// rather than six `with_` methods that each assign one.
pub(crate) struct Stores {
    /// `None` is a chain this deployment has never indexed.
    pub(crate) sync: Option<SyncStatus>,
    pub(crate) page: PositionPage,
    pub(crate) labels: HashMap<Address, TokenLabel>,
    pub(crate) prices: HashMap<ReserveKey, ReservePrice>,
    pub(crate) staleness: Staleness,

    pub(crate) request_timeout: Duration,

    /// Makes the position store accept the read and never answer it, which
    /// the port-1 pool cannot express: that is a dependency which is *down*.
    pub(crate) positions_hang: bool,

    /// How many times the price store was asked; the point is the answer `0`,
    /// because an explicit `as_of` skips the read rather than discarding it.
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
            request_timeout: PATIENT,
            positions_hang: false,
            price_reads: Arc::new(AtomicUsize::new(0)),
        }
    }
}

/// A deadline a case will not reach by accident.
pub(crate) const PATIENT: Duration = Duration::from_secs(30);

/// A fixed instant, so a body a case asserts whole does not move with the clock.
/// 2026-07-25T17:20:00Z.
pub(crate) const VALUED_AT: u64 = 1_785_000_000;

pub(crate) fn synced() -> SyncStatus {
    SyncStatus {
        chain_id: ChainId::known(1),
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
            probes: crate::probes::from_pool(clickhouse(), postgres),
            positions: Box::new(Positions {
                page: self.page,
                hang: self.positions_hang,
            }),
            tokens: Box::new(Labels(self.labels)),
            prices: Box::new(Prices {
                latest: self.prices,
                reads: Arc::clone(&self.price_reads),
            }),
            sync: Box::new(Synced(self.sync)),
            signer: Signer::new(SECRET).expect("HMAC takes a key of any length"),
            staleness: self.staleness,
            request_timeout: self.request_timeout,
            prefix: "api".to_owned(),
            docs_path: "docs".to_owned(),
            docs_assets: "/usr/share/api/docs".to_owned(),
        }
    }

    pub(crate) fn handler(self) -> Router {
        app::handler(self.app(postgres()))
    }
}

struct Positions {
    page: PositionPage,
    hang: bool,
}

#[async_trait::async_trait]
impl PositionStore for Positions {
    async fn list(&self, _query: &PositionQuery) -> Result<PositionPage, StoreError> {
        if self.hang {
            std::future::pending::<()>().await;
        }

        Ok(self.page.clone())
    }
}

struct Labels(HashMap<Address, TokenLabel>);

#[async_trait::async_trait]
impl TokenMetadataStore for Labels {
    async fn labels(&self, _chain_id: ChainId) -> Result<HashMap<Address, TokenLabel>, LabelError> {
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
        _chain_id: ChainId,
    ) -> Result<HashMap<ReserveKey, ReservePrice>, PriceError> {
        self.reads.fetch_add(1, Ordering::Relaxed);

        Ok(self.latest.clone())
    }
}

struct Synced(Option<SyncStatus>);

#[async_trait::async_trait]
impl SyncStatusStore for Synced {
    async fn get(&self, _chain_id: ChainId) -> Result<Option<SyncStatus>, SyncError> {
        Ok(self.0.clone())
    }
}

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

pub(crate) async fn answered(error: ApiError) -> (StatusCode, String) {
    let response = error.into_response();

    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

    (status, String::from_utf8(body.to_vec()).unwrap())
}
