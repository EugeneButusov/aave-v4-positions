//! How this service represents a failure, and what a caller learns from one.
//!
//! A trait object rather than a type, which is
//! [crates.io's](https://github.com/rust-lang/crates.io/blob/main/src/util/errors.rs)
//! shape: one blanket impl turns any `std` error into a logged 500, so a
//! handler can `?` a store read without writing a conversion, and the
//! constructors beside it demote the failures a caller caused. Two jobs in that
//! order, which is what Nest's exception filter does and what replaces it.
//!
//! **The envelope lives in [`json`], not here**, and that boundary is
//! crates.io's too. This module is the vocabulary of failures — what can go
//! wrong and what status it deserves. That one is the wire contract, which is
//! measured against the TypeScript and changes on a different schedule.
//!
//! **The 500 is fixed text, not the envelope**, which is crates.io's choice here
//! and also the one already made beside it: `CatchPanicLayer` answers a panic
//! the same way. The reason is that the TypeScript's 500 body is *unmeasured* —
//! Nest's default filter emits a different shape for an unhandled throw than for
//! an `HttpException`, with no `error` key and the remaining two the other way
//! round, and nothing in this tree records which. Writing it from memory of
//! Nest's source is the mistake [`json`]'s measured note exists to prevent, so
//! the shape lands with the first route that can fail, measured then.

mod json;

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Anything that knows what it looks like to a caller.
///
/// `response` takes `&self` rather than consuming, as crates.io's does: it is
/// what lets the error be logged and answered from the same value, and it is
/// the difference between this and a plain `IntoResponse` impl.
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

/// What this deployment has never heard of.
///
/// The message is the caller's, which is what makes it useful and what makes it
/// worth being careful about: it is echoed from the request line, so it reaches
/// a log and a browser. Nothing else of ours goes into it.
pub(crate) fn not_found(message: impl Into<Cow<'static, str>>) -> BoxedAppError {
    json::custom(StatusCode::NOT_FOUND, message)
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
