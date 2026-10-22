//! Everything wrapped around whatever this service serves.
//!
//! One place for the stack, after crates.io's, and taking its
//! `CatchPanicLayer` with it.
//!
//! The observation layer is outermost, and that ordering is the contract: a
//! request id prefers the trace id, so the span exists before the id is minted.

use std::any::Any;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::error_handling::HandleErrorLayer;
use axum::http::{Request, StatusCode, header};
use axum::response::{IntoResponse, Response};
use tower::timeout::TimeoutLayer;
use tower::timeout::error::Elapsed;
use tower::{BoxError, ServiceBuilder};
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{
    MakeRequestId, MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer,
};

use crate::requests::Instruments;

pub(crate) fn apply(router: Router, timeout: Duration) -> Router {
    let instruments = Instruments::new();

    router.layer(
        // The order is the contract: span outermost so the id is minted inside
        // a traced request — and so a deadline below is traced and counted like
        // any other answer — then the header, then its copy onto the way out,
        // and `CatchPanic` innermost so its 500 travels out through all three.
        ServiceBuilder::new()
            .layer(axum::middleware::from_fn_with_state(
                instruments,
                Instruments::observe,
            ))
            .layer(SetRequestIdLayer::x_request_id(Traced))
            .layer(PropagateRequestIdLayer::x_request_id())
            .layer(HandleErrorLayer::new(refused))
            .layer(TimeoutLayer::new(timeout))
            .layer(CatchPanicLayer::custom(panicked)),
    )
}

/// What an elapsed deadline is on the wire.
///
/// `504` rather than `408`: `408` says the *client* was too slow to send, and
/// nothing here measures that. What elapsed is our own wait on a dependency,
/// which is what `504` names.
async fn refused(error: BoxError) -> Response {
    let (status, detail) = if error.is::<Elapsed>() {
        (StatusCode::GATEWAY_TIMEOUT, "the request took too long")
    } else {
        // Unreachable: the layer below errors with `Elapsed` and nothing else.
        // Named rather than unwrapped so a second layer added here cannot turn
        // this into a panic by arriving.
        (StatusCode::INTERNAL_SERVER_ERROR, "the request failed")
    };

    tracing::warn!(status = status.as_u16(), detail, "request refused");

    (status, detail).into_response()
}

/// The trace id when there is one, a fresh UUID when there is not.
#[derive(Clone, Copy)]
struct Traced;

impl MakeRequestId for Traced {
    fn make_request_id<B>(&mut self, request: &Request<B>) -> Option<RequestId> {
        // Only reached when the caller sent none; `SetRequestId` keeps theirs,
        // which is stable across a retry where a trace id is not.
        telemetry::trace_id(&tracing::Span::current())
            .and_then(|id| id.parse().ok())
            .map(RequestId::new)
            .or_else(|| MakeRequestUuid.make_request_id(request))
    }
}

/// A panic is a bug here, and the caller learns nothing about it.
///
/// The body is fixed text, because a panic message carries whatever was in
/// scope when it fired. What an operator needs goes to the log instead — the
/// default hook writes plain text into a stream of JSON objects, with nothing
/// tying it to the request.
fn panicked(payload: Box<dyn Any + Send + 'static>) -> Response {
    // `as_ref()`, not `&payload`: a `&Box<dyn Any>` unsizes to a `&dyn Any`
    // holding the box, so every downcast below misses and every panic is
    // logged as having no message. Which is what the first version did.
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
    use axum::http::Request;

    use super::*;
    use crate::test_support::app::{get, handler, postgres, unreachable_postgres};
    use crate::test_support::observability::{Captured, interesting, observed};

    fn payload(value: impl Any + Send) -> Box<dyn Any + Send> {
        Box::new(value)
    }

    #[tokio::test]
    async fn a_panicking_handler_answers_500_and_says_nothing_about_why() {
        // Without the layer the connection drops with no response, which a
        // caller cannot tell from the service being down.
        async fn boom() -> &'static str {
            panic!("a secret only the server should know")
        }

        let boom = apply(
            Router::new().route("/boom", axum::routing::get(boom)),
            crate::test_support::app::PATIENT,
        );
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
        // Through both request-id layers, so the 500 is findable in the log.
        assert_eq!(request_id.as_deref(), Some("carried-through"));
    }

    #[tokio::test]
    async fn mints_a_request_id_for_a_caller_that_sent_none() {
        let request = Request::builder()
            .uri("/health/live")
            .body(Body::empty())
            .unwrap();

        let (_, _, request_id) = get(handler(postgres()), request).await;

        assert!(
            request_id.is_some_and(|id| !id.is_empty()),
            "no x-request-id on the response"
        );
    }

    #[tokio::test]
    async fn echoes_the_request_id_a_caller_did_send() {
        // Caller-supplied and stable across a retry, unlike a trace id.
        let request = Request::builder()
            .uri("/health/live")
            .header("x-request-id", "from-the-caller")
            .body(Body::empty())
            .unwrap();

        let (_, _, request_id) = get(handler(postgres()), request).await;

        assert_eq!(request_id.as_deref(), Some("from-the-caller"));
    }

    #[test]
    fn the_request_id_a_caller_gets_back_is_the_trace_id() {
        // Or a line carries a `request_id` and a `trace_id` with nothing to do
        // with each other, and an echoed header naming neither.
        let request = Request::builder()
            .uri("/api/v1/chains/1/users/0x0000000000000000000000000000000000000001/positions")
            .body(Body::empty())
            .unwrap();

        let observed = observed(handler(unreachable_postgres()), request);
        let trace = observed.spans[0].span_context.trace_id().to_string();

        assert_eq!(observed.request_id.as_deref(), Some(trace.as_str()));
    }

    #[test]
    fn a_caller_s_own_request_id_outranks_the_trace_id() {
        let request = Request::builder()
            .uri("/api/v1/chains/1/users/0x0000000000000000000000000000000000000001/positions")
            .header("x-request-id", "from-the-caller")
            .body(Body::empty())
            .unwrap();

        let observed = observed(handler(unreachable_postgres()), request);

        assert_eq!(observed.request_id.as_deref(), Some("from-the-caller"));
        assert!(
            observed
                .logged
                .contains(r#""request_id":"from-the-caller""#),
            "{}",
            observed.logged
        );
    }

    /// What the panic handler wrote, as a deployment would read it. Asserting
    /// on [`said`] alone leaves the coercion uncovered, which is how the bug
    /// this catches reached a running process.
    fn logged_by(panic: Box<dyn Any + Send>) -> String {
        interesting();
        let (captured, lines) = Captured::new();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(captured)
            .finish();

        tracing::subscriber::with_default(subscriber, || drop(panicked(panic)));

        lines.read()
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
        // `panic!("x")` is a `&str` and `panic!("{x}")` a `String`; reading
        // one of the two loses half the messages to the fallback.
        assert_eq!(said(&*payload("a literal")), "a literal");
        assert_eq!(
            said(&*payload(format!("a {} one", "formatted"))),
            "a formatted one"
        );
        assert_eq!(said(&*payload(42_u8)), "a panic carrying no message");
    }
}
