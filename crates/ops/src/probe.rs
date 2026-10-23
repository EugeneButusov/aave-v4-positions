//! The two probe routes, and the paths they are pinned to.
//!
//! The paths live here rather than at each call site: a compose healthcheck, a
//! Kubernetes manifest and a load balancer are all written against them, so a
//! second spelling is a deployment that fails where no test would see it.
//!
//! What to check is the caller's, passed as one closure naming its dependencies
//! — Quickwit's shape, with the aggregation and status mapping factored here.

use std::time::Instant;

use axum::routing::get;
use axum::{Json, Router, http::StatusCode};

use crate::health::{Alive, CheckResult, Liveness, Readiness, Report};
use crate::shutdown::ShutdownFlag;

/// What both probe paths begin with.
const HEALTH_PREFIX: &str = "/health";

/// Whether a path is one of the probes, so a caller can drop it from the
/// request log and the traces without burying anything else.
///
/// On a segment boundary. A raw prefix test also swallowed `/healthz` and
/// `/health-report`, which are somebody's routes and belong in the log.
#[must_use]
pub fn is_probe(path: &str) -> bool {
    match path.strip_prefix(HEALTH_PREFIX) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

/// When the process started. A type rather than a bare `Instant`, because
/// building the router is already too late to start counting.
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
/// Unversioned and outside any API prefix, so deployment manifests do not
/// depend on how the API is versioned.
pub fn probe_router<F, Fut>(uptime: Uptime, shutdown: ShutdownFlag, checks: F) -> Router
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
                let (shutdown, checks) = (shutdown.clone(), checks.clone());
                async move {
                    let report = Report::new(checks().await, shutdown.has_begun());
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
    //! Against the bytes, and the bytes were measured: every expectation was
    //! captured by curling the TypeScript service — with ClickHouse reachable,
    //! pointed at a closed port, and after a SIGTERM.

    #[test]
    fn counts_a_probe_path_and_not_a_path_that_merely_starts_like_one() {
        for probe in ["/health", "/health/", "/health/live", "/health/ready"] {
            assert!(super::is_probe(probe), "{probe}");
        }

        for route in [
            "/healthz",
            "/health-report",
            "/healthjunk",
            "/api/health",
            "",
        ] {
            assert!(!super::is_probe(route), "{route}");
        }
    }

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
    fn router(shutdown: ShutdownFlag, checks: Vec<CheckResult>) -> Router {
        probe_router(Uptime::now(), shutdown, move || {
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
    async fn both_probes_answer_under_the_prefix_callers_exclude() {
        // The routes above stay literals; this keeps the prefix on them.
        for probe in ["live", "ready"] {
            let path = format!("{HEALTH_PREFIX}/{probe}");
            let (status, _) = request(router(ShutdownFlag::new(), vec![]), &path).await;

            assert_eq!(status, StatusCode::OK, "{path}");
        }
    }

    #[tokio::test]
    async fn liveness_reports_ok_and_an_uptime() {
        let (status, body) = request(router(ShutdownFlag::new(), vec![]), "/health/live").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, r#"{"status":"ok","uptime_seconds":0}"#);
    }

    #[tokio::test]
    async fn liveness_answers_while_draining() {
        // Measured: the TypeScript answers 200 through the whole grace window.
        // Failing it would have kubelet restart a pod that is draining.
        let shutdown = ShutdownFlag::new();
        shutdown.begin();

        let (status, _) = request(router(shutdown, vec![up("clickhouse")]), "/health/live").await;

        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn readiness_is_ok_with_every_dependency_up() {
        let checks = vec![up("clickhouse"), up("postgres")];

        let (status, body) = request(router(ShutdownFlag::new(), checks), "/health/ready").await;

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

        let (status, body) = request(router(ShutdownFlag::new(), checks), "/health/ready").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        // The report and nothing else — no `status_code`, `message` or `error`
        // wrapper. Measured, because no TypeScript spec covers it.
        assert_eq!(
            body,
            r#"{"status":"degraded","checks":[{"name":"clickhouse","status":"down","error":"connect ECONNREFUSED 127.0.0.1:9"},{"name":"postgres","status":"up"}]}"#
        );
    }

    #[tokio::test]
    async fn readiness_fails_while_draining_with_everything_up() {
        let shutdown = ShutdownFlag::new();
        shutdown.begin();
        let checks = vec![up("clickhouse"), up("postgres")];

        let (status, body) = request(router(shutdown, checks), "/health/ready").await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            body,
            r#"{"status":"shutting_down","checks":[{"name":"clickhouse","status":"up"},{"name":"postgres","status":"up"}]}"#
        );
    }

    #[tokio::test]
    async fn readiness_still_reports_the_checks_while_draining() {
        // Measured: the checks keep running and a failing one is still named.
        let shutdown = ShutdownFlag::new();
        shutdown.begin();
        let checks = vec![down("clickhouse", "connection refused"), up("postgres")];

        let (_, body) = request(router(shutdown, checks), "/health/ready").await;

        assert!(
            body.contains(r#"{"name":"clickhouse","status":"down","error":"connection refused"}"#)
        );
    }
}
