//! How this service represents a failure, and what a caller learns from one.
//!
//! A trait object rather than a type: one blanket impl turns any `std` error
//! into a logged 500, and the constructors beside it demote the failures a
//! caller caused.
//!
//! The envelope lives in [`json`] — this module is the vocabulary of failures,
//! that one is the wire contract. A 5xx answers fixed text rather than the
//! envelope; `docs/rust-migration.md` records the difference.

mod json;

pub(crate) use json::ApiErrorResponse;

use std::error::Error;
use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Anything that knows what it looks like to a caller.
///
/// `response` takes `&self` so an error can be logged and answered from the
/// same value, which a plain `IntoResponse` impl could not.
pub(crate) trait AppError: Send + fmt::Display + fmt::Debug + 'static {
    fn response(&self) -> Response;
}

/// Box allocated, because the point is that the set of failures is open.
pub(crate) type BoxedAppError = Box<dyn AppError>;

impl AppError for BoxedAppError {
    fn response(&self) -> Response {
        (**self).response()
    }
}

impl IntoResponse for BoxedAppError {
    fn into_response(self) -> Response {
        self.response()
    }
}

/// Every `std` error, answered as a 500 that says nothing about itself.
///
/// The conversions that remain are the ones that improve on a 500 — a missing
/// row is a 404, a valuation that refuses is a 400 — so their absence is a
/// default rather than a gap. What the error said goes to the log instead:
/// this service answers for any wallet anyone asks about.
impl<E: Error + Send + 'static> AppError for E {
    fn response(&self) -> Response {
        // The chain, not just the head: a store names its own stage and leaves
        // the cause to `source()`, so `%self` never says which half failed.
        tracing::error!(error = %ops::error_message_with_causes(self), "a request failed");

        (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
    }
}

/// Every `std` error becomes one, so a handler can `?` a store read. No overlap
/// with the impl above: `dyn AppError` is not a `std::error::Error`, so
/// `From<T> for T` still applies to `BoxedAppError`.
impl<E: Error + Send + 'static> From<E> for BoxedAppError {
    fn from(error: E) -> Self {
        Box::new(error)
    }
}

/// What this deployment has never heard of.
///
/// The message is echoed from the request line, so it reaches a log and a
/// browser. Nothing else of ours goes into it.
pub(crate) fn not_found(message: String) -> BoxedAppError {
    json::custom(StatusCode::NOT_FOUND, message)
}

/// What the caller got wrong, said back to them. Without it a malformed
/// address answers a 500, telling a caller their mistake is the server's.
pub(crate) fn bad_request(message: String) -> BoxedAppError {
    json::custom(StatusCode::BAD_REQUEST, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::answered;

    #[tokio::test]
    async fn a_std_error_is_a_500_that_tells_the_caller_nothing() {
        // Why the blanket impl answers fixed text: this message names an
        // internal host, which forwarding `Display` would put on the wire.
        let failed = std::io::Error::other("connection refused: clickhouse.internal:8123");
        let (status, body) = answered(Box::new(failed)).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, "Internal Server Error");
        assert!(
            !body.contains("clickhouse.internal"),
            "the cause reached the wire: {body}"
        );
    }

    /// A store's own message, with the half that failed underneath it.
    #[derive(Debug, thiserror::Error)]
    #[error("the sync status read failed")]
    struct Read(#[source] std::io::Error);

    #[test]
    fn the_log_line_carries_the_cause_and_the_wire_still_does_not() {
        // `interesting` because the case above reaches this same callsite with
        // nothing in scope, which settles it off for the process.
        crate::test_support::interesting();
        let written = crate::test_support::Written::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(written.clone())
            .finish();
        let failed = Read(std::io::Error::other("Connection refused"));

        let (_, body) = tracing::subscriber::with_default(subscriber, || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(answered(Box::new(failed)))
        });
        let logged = written.read();

        assert!(logged.contains("the sync status read failed"), "{logged}");
        assert!(logged.contains("Connection refused"), "{logged}");
        assert_eq!(body, "Internal Server Error");
    }
}
