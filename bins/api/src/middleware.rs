//! Everything wrapped around whatever this service serves.
//!
//! One place for the stack, as
//! [crates.io's](https://github.com/rust-lang/crates.io/blob/main/src/middleware.rs) is — and taking
//! its `CatchPanicLayer` with it. Theirs takes the state too, for the
//! `from_fn_with_state` middleware it has; this one has none yet and a parameter
//! nothing reads is `dead_code`, which the workspace denies.
//!
//! **No `TraceLayer` yet, and that is not a gap.** The TypeScript excludes
//! `/health` from request logging, so a process serving only probes emits
//! exactly what its predecessor does for the same traffic — nothing. It lands
//! with the first route worth tracing, where the exclusion has something to
//! exclude.

use std::any::Any;

use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use tower::ServiceBuilder;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};

pub(crate) fn apply(router: Router) -> Router {
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
    use axum::http::Request;

    use super::*;
    use crate::test_support::{get, postgres};

    /// Boxed, because that is how a payload arrives.
    fn payload(value: impl Any + Send) -> Box<dyn Any + Send> {
        Box::new(value)
    }

    #[tokio::test]
    async fn a_panicking_handler_answers_500_and_says_nothing_about_why() {
        // Without the layer this drops the connection with no response at all,
        // which a caller cannot tell from the service being down.
        async fn boom() -> &'static str {
            panic!("a secret only the server should know")
        }

        let boom = apply(Router::new().route("/boom", axum::routing::get(boom)));
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

    #[tokio::test]
    async fn mints_a_request_id_for_a_caller_that_sent_none() {
        let request = Request::builder()
            .uri("/health/live")
            .body(Body::empty())
            .unwrap();

        let (_, _, request_id) = get(crate::test_support::handler(postgres()), request).await;

        assert!(
            request_id.is_some_and(|id| !id.is_empty()),
            "no x-request-id on the response"
        );
    }

    #[tokio::test]
    async fn echoes_the_request_id_a_caller_did_send() {
        // The header is caller-supplied and stable across a retry, which is what
        // makes a client-side report findable in the log stream.
        let request = Request::builder()
            .uri("/health/live")
            .header("x-request-id", "from-the-caller")
            .body(Body::empty())
            .unwrap();

        let (_, _, request_id) = get(crate::test_support::handler(postgres()), request).await;

        assert_eq!(request_id.as_deref(), Some("from-the-caller"));
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
}
