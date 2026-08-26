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
//!
//! **It is handed its dependencies rather than making them**, and each of the
//! four has its own reason. The [`Drain`] has two holders — the readiness
//! handler and the future `with_graceful_shutdown` waits on — so one made in
//! here could never be flipped by the other. [`Uptime`] is read at the top of
//! `main`, because started here it would begin counting after the config and
//! both clients. The clients could be built from a [`crate::config::Config`]
//! and are not, because they are about to have a second consumer each: the
//! position store takes the ClickHouse client, the read stores take the pool,
//! and a `router` that constructed them would have to construct those too — at
//! which point it is the composition root rather than a description of what is
//! served. It is also what lets the cases below hand it a Postgres pool pointed
//! at a closed port, and what will let them hand it a fake store instead of a
//! doctored URL.

use std::any::Any;

use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use clickhouse_client::clickhouse::Client;
use ops::{Drain, Uptime};
use postgres::Pool;
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};

pub(crate) fn router(uptime: Uptime, drain: Drain, clickhouse: Client, postgres: Pool) -> Router {
    let probes = ops::probe_router(uptime, drain, move || {
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
    });

    served(probes)
}

/// Everything wrapped around whatever this service serves.
///
/// A function rather than a chain inline, so the cases below can put a route
/// that misbehaves underneath the same stack the real one runs.
fn served(router: Router) -> Router {
    router.layer(
        // Ordered explicitly, because it has to be: `SetRequestId` is outermost
        // so the header exists before anything downstream reads it,
        // `PropagateRequestId` sits inside it to copy the value onto the way
        // out, and `CatchPanic` is innermost so the 500 it makes still travels
        // out through both and carries the id. Reversed, every response would
        // carry a fresh id unrelated to the one the caller sent.
        ServiceBuilder::new()
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .layer(PropagateRequestIdLayer::x_request_id())
            .layer(CatchPanicLayer::custom(panicked)),
    )
}

/// A panic is a bug here, and the caller learns nothing about it.
///
/// **The body is fixed text.** A panic message carries whatever was in scope
/// when it fired, which is not a thing to hand a stranger — and this service
/// answers for any wallet anyone asks about, so every caller is one.
///
/// **What an operator needs goes to the log instead.** The default hook already
/// writes the panic to stderr, but as plain text in a stream where every other
/// line is a JSON object, and with nothing tying it to the request. This is the
/// same fact as a record.
fn panicked(payload: Box<dyn Any + Send + 'static>) -> Response {
    // `as_ref()` rather than `&payload`, and it is not a style choice: a
    // `&Box<dyn Any>` unsizes to a `&dyn Any` holding the *box*, so every
    // downcast below misses and every panic is logged as having no message.
    // That is not hypothetical — it is what the first version of this did, and
    // only running it found out.
    tracing::error!(panic = said(payload.as_ref()), "a handler panicked");

    let mut response = Response::new(Body::from("Service panicked"));
    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// What `panic!` was given, which is a `String` when it was formatted and a
/// `&str` when it was a literal. Anything else is a payload nobody here creates.
fn said(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&'static str>().copied())
        .unwrap_or("a panic carrying no message")
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
    async fn a_panicking_handler_answers_500_and_says_nothing_about_why() {
        // Without the layer this drops the connection with no response at all,
        // which a caller cannot tell from the service being down.
        async fn boom() -> &'static str {
            panic!("a secret only the server should know")
        }

        let boom = served(Router::new().route("/boom", axum::routing::get(boom)));
        let request = Request::builder()
            .uri("/boom")
            .header("x-request-id", "carried-through")
            .body(Body::empty())
            .unwrap();

        let (status, body, request_id) = get(boom, request).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, "Service panicked");
        assert!(
            !body.contains("secret"),
            "the panic message reached the wire"
        );
        // Still through both request-id layers, so the 500 is findable in the
        // log beside the record the handler wrote.
        assert_eq!(request_id.as_deref(), Some("carried-through"));
    }

    /// Boxed, because that is how a payload arrives.
    fn payload(value: impl Any + Send) -> Box<dyn Any + Send> {
        Box::new(value)
    }

    /// Collects what a subscriber was given, so a case can assert on a log line.
    ///
    /// Worth the twenty lines: reading the message is done at one call site with
    /// a coercion that can silently pick the wrong thing, and asserting on
    /// [`said`] alone leaves that call site uncovered — which is exactly how the
    /// bug this catches got as far as a running process.
    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn logged_by(panic: Box<dyn Any + Send>) -> String {
        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(captured.clone())
            .finish();

        tracing::subscriber::with_default(subscriber, || drop(panicked(panic)));

        let written = captured.0.lock().unwrap().clone();
        String::from_utf8(written).unwrap()
    }

    #[test]
    fn logs_the_panic_message_rather_than_the_box_around_it() {
        let logged = logged_by(payload("a secret only the server should know"));

        assert!(
            logged.contains(r#""panic":"a secret only the server should know""#),
            "{logged}"
        );
    }

    #[test]
    fn reads_a_panic_message_whether_it_was_formatted_or_a_literal() {
        // `panic!("x")` hands over a `&str` and `panic!("{x}")` a `String`;
        // reading only one of the two loses half the messages to the fallback.
        assert_eq!(said(&*payload("a literal")), "a literal");
        assert_eq!(
            said(&*payload(format!("a {} one", "formatted"))),
            "a formatted one"
        );
        assert_eq!(said(&*payload(42_u8)), "a panic carrying no message");
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
