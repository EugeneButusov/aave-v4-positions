//! One `App` from the environment's defaults, so a case says what it is testing.
//!
//! [crates.io's `TestApp`](https://github.com/rust-lang/crates.io/blob/main/src/tests/util/test_app.rs)
//! at this service's scale: the cases below drive the real router rather than
//! reassembling one, so a wiring mistake fails them rather than hiding behind a
//! parallel definition.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use clickhouse_client::clickhouse::Client;
use ops::{Drain, Uptime};
use postgres::Pool;
use tower::ServiceExt;

use crate::app::{self as app, App};

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

pub(crate) fn state(postgres: Pool) -> Arc<App> {
    Arc::new(App {
        uptime: Uptime::now(),
        drain: Drain::new(),
        clickhouse: clickhouse(),
        postgres,
    })
}

/// The router this binary actually serves, over the given Postgres.
pub(crate) fn handler(postgres: Pool) -> Router {
    app::handler(state(postgres))
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
