//! What this service is, as one router.
//!
//! Probes and nothing else, for now. The versioned routes and the prefix they
//! sit under arrive with the positions endpoint; a prefix with nothing beneath
//! it is configuration that cannot be observably wrong.
//!
//! **The two dependencies are named here, in a list, and that is the whole
//! registry.** `ops` owns the report and the paths; this owns which databases
//! this process is answering for, which is knowledge the composition root
//! already has and nothing else needs.

use axum::Router;
use clickhouse_client::clickhouse::Client;
use ops::{Drain, Uptime};
use postgres::Pool;
use tower::ServiceBuilder;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};

pub(crate) fn router(uptime: Uptime, drain: Drain, clickhouse: Client, postgres: Pool) -> Router {
    ops::probe_router(uptime, drain, move || {
        let (clickhouse, postgres) = (clickhouse.clone(), postgres.clone());
        async move {
            // Side by side rather than one after the other, as the TypeScript's
            // `Promise.all` does: neither answer depends on the other, and a
            // probe that serialises them reports the sum of two timeouts.
            // `join!` keeps the order of the results, which the wire contract
            // fixes.
            let (clickhouse, postgres) = tokio::join!(
                ops::check("clickhouse", clickhouse_client::ping(&clickhouse)),
                ops::check("postgres", ::postgres::ping(&postgres)),
            );
            vec![clickhouse, postgres]
        }
    })
    .layer(
        // Ordered explicitly, because it has to be: `SetRequestId` is outermost
        // so the header exists before anything downstream reads it, and
        // `PropagateRequestId` sits inside it to copy the value onto the way
        // out. Reversed, every response would carry a fresh id unrelated to the
        // one the caller sent.
        ServiceBuilder::new()
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .layer(PropagateRequestIdLayer::x_request_id()),
    )
}

#[cfg(test)]
mod tests {
    //! Against real servers, because the wiring is the only thing left to get
    //! wrong: `ops` already proves the report and the drain in isolation, and
    //! what these add is that the names, the order and the two `ping`s behind
    //! them are hooked up to the databases this process actually opens.

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::*;

    /// The same defaults every other suite in this workspace reads.
    fn clickhouse() -> Client {
        clickhouse_client::build_client(clickhouse_client::Config {
            url: std::env::var("CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".to_owned()),
            database: "default".to_owned(),
            user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
            password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
        })
    }

    fn postgres() -> Pool {
        let url = std::env::var("POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned());

        ::postgres::build_pool(&url).unwrap()
    }

    /// Port 1 is reserved and nothing listens on it, so the pool's first dial
    /// is refused — a dependency that is down rather than one that is slow.
    fn unreachable_postgres() -> Pool {
        ::postgres::build_pool("postgres://postgres@127.0.0.1:1/postgres").unwrap()
    }

    fn app(postgres: Pool) -> Router {
        router(Uptime::now(), Drain::new(), clickhouse(), postgres)
    }

    async fn get(router: Router, request: Request<Body>) -> (StatusCode, String, Option<String>) {
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

    fn ready() -> Request<Body> {
        Request::builder()
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn reports_both_databases_up_against_the_real_servers() {
        let (status, body, _) = get(app(postgres()), ready()).await;

        assert_eq!(status, StatusCode::OK);
        // Byte for byte what the TypeScript service answered when both were
        // reachable, measured rather than transcribed from its DTOs.
        assert_eq!(
            body,
            r#"{"status":"ok","checks":[{"name":"clickhouse","status":"up"},{"name":"postgres","status":"up"}]}"#
        );
    }

    #[tokio::test]
    async fn names_postgres_when_only_postgres_is_unreachable() {
        // Which is the half a single aggregated boolean would lose, and the
        // reason the report carries names at all.
        let (status, body, _) = get(app(unreachable_postgres()), ready()).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.starts_with(r#"{"status":"degraded","#), "{body}");
        assert!(
            body.contains(r#"{"name":"clickhouse","status":"up"}"#),
            "{body}"
        );
        assert!(
            body.contains(r#"{"name":"postgres","status":"down","error":"#),
            "{body}"
        );
    }

    #[tokio::test]
    async fn mints_a_request_id_for_a_caller_that_sent_none() {
        let request = Request::builder()
            .uri("/health/live")
            .body(Body::empty())
            .unwrap();

        let (_, _, request_id) = get(app(postgres()), request).await;

        assert!(
            request_id.is_some_and(|id| !id.is_empty()),
            "no x-request-id on the response"
        );
    }

    #[tokio::test]
    async fn echoes_the_request_id_a_caller_did_send() {
        // The header is caller-supplied and stable across a retry, which is
        // what makes a client-side report findable in the log stream.
        let request = Request::builder()
            .uri("/health/live")
            .header("x-request-id", "from-the-caller")
            .body(Body::empty())
            .unwrap();

        let (_, _, request_id) = get(app(postgres()), request).await;

        assert_eq!(request_id.as_deref(), Some("from-the-caller"));
    }

    /// The one case that goes through a socket rather than around it.
    ///
    /// Everything above drives the router directly, which leaves `axum::serve`,
    /// the listener and the shutdown future untested — and those are `main`'s
    /// whole contribution. One request over TCP, then the drain, covers it
    /// without an HTTP client dependency: the response is read as bytes because
    /// the only thing being asserted is that the process answered.
    #[tokio::test]
    async fn serves_over_a_socket_and_stops_when_drained() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let drain = Drain::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        // Stands in for the signal, so the drain happens after the request
        // rather than racing it.
        let (terminate, terminated) = tokio::sync::oneshot::channel::<()>();

        let served = tokio::spawn({
            let app = router(Uptime::now(), drain.clone(), clickhouse(), postgres());
            let drain = drain.clone();
            async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async move {
                        let _ = terminated.await;
                        drain.begin_and_hold(std::time::Duration::ZERO).await;
                    })
                    .await
                    .unwrap();
            }
        });

        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket
            .write_all(b"GET /health/live HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();

        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(
            response.ends_with(r#"{"status":"ok","uptimeSeconds":0}"#),
            "{response}"
        );

        // Resolving the shutdown future is what ends the accept loop; a server
        // that ignored it would hang here rather than fail.
        terminate.send(()).unwrap();
        served.await.unwrap();
    }
}
