//! The two probe routes, and the paths they are pinned to.
//!
//! **The paths live here rather than at each call site.** They are what a
//! compose healthcheck, a Kubernetes manifest and a load balancer are all
//! written against, so a second service spelling them differently is a
//! deployment that fails in a way no test would see.
//!
//! What to check is the caller's, passed as one closure that names its
//! dependencies in a literal list. That is the shape
//! [Quickwit uses](https://github.com/quickwit-oss/quickwit/blob/main/quickwit/quickwit-serve/src/health_check_api/handler.rs) —
//! the real collaborators handed to the handler, no registry and no indirection
//! — with the aggregation and the status mapping factored out to here so two
//! binaries cannot disagree about them.

use std::time::Instant;

use axum::routing::get;
use axum::{Json, Router, http::StatusCode};

use crate::drain::Drain;
use crate::health::{Alive, CheckResult, Liveness, Readiness, Report};

/// When the process started, taken as early as `main` can take it.
///
/// A type rather than a bare `Instant` parameter, because the value only means
/// anything if it was read at the top of `main` — building the router is
/// already several connections too late to start counting.
#[derive(Clone, Copy, Debug)]
pub struct Uptime(Instant);

impl Uptime {
    #[must_use]
    pub fn now() -> Self {
        Self(Instant::now())
    }

    fn seconds(self) -> u64 {
        self.0.elapsed().as_secs()
    }
}

/// `/health/live` and `/health/ready`, ready to merge into a service's router.
///
/// Unversioned and outside any API prefix, deliberately: probe paths are
/// infrastructure, and pinning them keeps deployment manifests independent of
/// how the API is versioned.
pub fn probe_router<F, Fut>(uptime: Uptime, drain: Drain, checks: F) -> Router
where
    F: Fn() -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Vec<CheckResult>> + Send + 'static,
{
    Router::new()
        .route(
            "/health/live",
            get(move || async move {
                Json(Liveness {
                    status: Alive::Ok,
                    uptime_seconds: uptime.seconds(),
                })
            }),
        )
        .route(
            "/health/ready",
            get(move || {
                let (drain, checks) = (drain.clone(), checks.clone());
                async move {
                    let report = Report::new(checks().await, drain.is_draining());
                    (code(report.status), Json(report))
                }
            }),
        )
}

/// Matched exhaustively so a new status has to decide its code here.
fn code(status: Readiness) -> StatusCode {
    match status {
        Readiness::Ok => StatusCode::OK,
        Readiness::Degraded | Readiness::ShuttingDown => StatusCode::SERVICE_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    //! Against the bytes, and the bytes were measured.
    //!
    //! Every expectation below was captured by curling the TypeScript service
    //! this replaces — running against a real Postgres, once with ClickHouse
    //! reachable and once pointed at a closed port, and once after a SIGTERM.
    //! Reading them off its DTO classes would have missed the one thing no
    //! TypeScript test pins: the 503 body is the report itself, with no
    //! framework envelope around it.

    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use tower::ServiceExt;

    use super::*;
    use crate::health::CheckStatus;

    fn up(name: &'static str) -> CheckResult {
        CheckResult {
            name,
            status: CheckStatus::Up,
            error: None,
        }
    }

    fn down(name: &'static str, error: &str) -> CheckResult {
        CheckResult {
            name,
            status: CheckStatus::Down,
            error: Some(error.to_owned()),
        }
    }

    /// A router over a fixed answer, so a case says what it is testing.
    fn router(drain: Drain, checks: Vec<CheckResult>) -> Router {
        probe_router(Uptime::now(), drain, move || {
            let checks = checks.clone();
            async move { checks }
        })
    }

    async fn request(router: Router, path: &str) -> (StatusCode, String) {
        let response = router
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();

        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn liveness_reports_ok_and_an_uptime() {
        let (status, body) = request(router(Drain::new(), vec![]), "/health/live").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"status":"ok","uptimeSeconds":0}"#);
    }

    #[tokio::test]
    async fn liveness_answers_while_draining() {
        // Measured: the TypeScript keeps answering 200 here through the whole
        // grace window. Failing it would have kubelet restart a pod that is
        // deliberately shutting down.
        let drain = Drain::new();
        drain.begin();

        let (status, _) = request(router(drain, vec![up("clickhouse")]), "/health/live").await;

        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn readiness_is_ok_with_every_dependency_up() {
        let checks = vec![up("clickhouse"), up("postgres")];

        let (status, body) = request(router(Drain::new(), checks), "/health/ready").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            r#"{"status":"ok","checks":[{"name":"clickhouse","status":"up"},{"name":"postgres","status":"up"}]}"#
        );
    }

    #[tokio::test]
    async fn readiness_names_the_dependency_that_is_down() {
        let checks = vec![
            down("clickhouse", "connect ECONNREFUSED 127.0.0.1:9"),
            up("postgres"),
        ];

        let (status, body) = request(router(Drain::new(), checks), "/health/ready").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        // The 503 body is the report and nothing else — no `statusCode`, no
        // `message`, no `error` wrapper. Measured, because the TypeScript
        // raises a framework exception here and no spec of its own covers it.
        assert_eq!(
            body,
            r#"{"status":"degraded","checks":[{"name":"clickhouse","status":"down","error":"connect ECONNREFUSED 127.0.0.1:9"},{"name":"postgres","status":"up"}]}"#
        );
    }

    #[tokio::test]
    async fn readiness_fails_while_draining_with_everything_up() {
        let drain = Drain::new();
        drain.begin();
        let checks = vec![up("clickhouse"), up("postgres")];

        let (status, body) = request(router(drain, checks), "/health/ready").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body,
            r#"{"status":"shutting_down","checks":[{"name":"clickhouse","status":"up"},{"name":"postgres","status":"up"}]}"#
        );
    }

    #[tokio::test]
    async fn readiness_still_reports_the_checks_while_draining() {
        // Measured against the TypeScript: the checks keep running through the
        // drain and a failing one is still named. Only the top line changes.
        let drain = Drain::new();
        drain.begin();
        let checks = vec![down("clickhouse", "connection refused"), up("postgres")];

        let (_, body) = request(router(drain, checks), "/health/ready").await;

        assert!(
            body.contains(r#"{"name":"clickhouse","status":"down","error":"connection refused"}"#)
        );
    }
}
