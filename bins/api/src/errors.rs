//! How this service represents a failure, and what a caller learns from one.
//!
//! A trait object rather than a type: one blanket impl turns any `std` error
//! into a logged 500, so a handler can `?` a store read without writing a
//! conversion, and the constructors beside it demote the failures a caller
//! caused.
//!
//! **The envelope lives in [`json`], not here.** This module is the vocabulary
//! of failures — what can go wrong and what status it deserves. That one is the
//! wire contract, and the two move on different schedules.
//!
//! **A 5xx answers fixed text rather than the envelope**, which is what
//! `CatchPanicLayer` beside it already does. Publishing a second error shape is
//! what [`json`]'s doc argues against; `docs/rust-migration.md` records what a
//! comparator should do with the difference.

mod json;

pub(crate) use json::ApiErrorResponse;

use std::error::Error;
use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Anything that knows what it looks like to a caller.
///
/// `response` takes `&self` rather than consuming, which is what lets an error
/// be logged and answered from the same value — the difference between this and
/// a plain `IntoResponse` impl.
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
/// **The blanket impl is the whole pattern.** Without it each new failure needs
/// a hand-written conversion before a handler can return it; with it the
/// conversions that remain are the ones that *improve* on a 500 — a missing row
/// is a 404, a valuation that refuses is a 400 — and their absence is a default
/// rather than a gap.
///
/// What the caller learns is the status. What the error said goes to the log,
/// for the same reason the panic handler keeps its payload off the wire: this
/// service answers for any wallet anyone asks about, so every caller is a
/// stranger.
impl<E: Error + Send + 'static> AppError for E {
    fn response(&self) -> Response {
        tracing::error!(error = %self, "a request failed");

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
/// The message is the caller's, which is what makes it useful and what makes it
/// worth being careful about: it is echoed from the request line, so it reaches
/// a log and a browser. Nothing else of ours goes into it.
pub(crate) fn not_found(message: String) -> BoxedAppError {
    json::custom(StatusCode::NOT_FOUND, message)
}

/// What the caller got wrong, said back to them.
///
/// Without it a malformed address reaches the blanket impl above and answers a
/// 500, telling a caller their own mistake is the server's fault. The message
/// names the parameter and quotes what was sent, which is safe to echo for the
/// reason [`not_found`]'s is: it came from the request line.
pub(crate) fn bad_request(message: String) -> BoxedAppError {
    json::custom(StatusCode::BAD_REQUEST, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::answered;

    #[tokio::test]
    async fn a_std_error_is_a_500_that_tells_the_caller_nothing() {
        // The blanket impl, and the reason it answers fixed text: this message
        // names an internal host, and a `?` in a handler is how it would reach
        // the wire if the impl simply forwarded `Display`.
        let failed = std::io::Error::other("connection refused: clickhouse.internal:8123");
        let (status, body) = answered(Box::new(failed)).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, "Internal Server Error");
        assert!(
            !body.contains("clickhouse.internal"),
            "the cause reached the wire: {body}"
        );
    }
}
