//! What a failure looks like on the wire.
//!
//! A trait object rather than a type, which is
//! [crates.io's](https://github.com/rust-lang/crates.io/blob/main/src/util/errors.rs)
//! shape: one blanket impl turns any `std` error into a logged 500, so a
//! handler can `?` a store read without writing a conversion, and the
//! constructors beside it demote the failures a caller caused. Two jobs in that
//! order, which is what Nest's exception filter does and what replaces it.
//!
//! **The envelope is Nest's, deliberately.** `api-error.dto.ts` explains why it
//! is not a custom one: the default filter already produces this shape for every
//! `HttpException` in the application, "including the ones Nest raises itself for
//! an unknown route — inventing a different one would mean either catching those
//! too or publishing a contract with two error shapes in it". crates.io's own
//! envelope is `{"errors":[{"detail":…}]}`, which is the registry API's
//! convention and not ours to borrow.
//!
//! **The field order is measured**, and it is not the order the DTO class
//! declares: the running service emits `message`, then `error`, then the status.
//! A byte-comparing gate sees the difference, so the order is the contract even
//! though the third key's spelling is not.
//!
//! **That third key is `status_code` where the TypeScript says `statusCode`**,
//! the same deliberate deviation the probe surface makes and for the same
//! reason — this port reads as Rust rather than as a transliteration. Unlike the
//! probes, this one is parsed by callers, so it is the kind of change that goes
//! in the release note rather than passing unnoticed. `docs/rust-migration.md`
//! names both keys as the differential's only expected differences.
//!
//! **The 500 is fixed text, not the envelope**, which is crates.io's choice here
//! and also the one already made beside it: `CatchPanicLayer` answers a panic
//! the same way. The reason is that the TypeScript's 500 body is *unmeasured* —
//! Nest's default filter emits a different shape for an unhandled throw than for
//! an `HttpException`, with no `error` key and the remaining two the other way
//! round, and nothing in this tree records which. Writing it from memory of
//! Nest's source is the mistake the measured note above exists to prevent, so
//! the shape lands with the first route that can fail, measured then.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Serialize, Serializer};

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

/// A failure the caller caused, carrying the status it deserves.
///
/// **Deliberately not a `std::error::Error`**, which is why `Display` is written
/// out rather than derived with `thiserror`: implementing it would overlap the
/// blanket impl above and the two would no longer be distinguishable. crates.io
/// hand-writes the same `Display` for the same reason.
#[derive(Debug)]
struct CustomApiError {
    status: StatusCode,
    message: Cow<'static, str>,
}

impl fmt::Display for CustomApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(f)
    }
}

impl AppError for CustomApiError {
    fn response(&self) -> Response {
        let body = ApiErrorResponse {
            message: &self.message,
            // Derived rather than passed, because Nest derives it: the `error`
            // key is the status's reason phrase and nothing else. Every status
            // constructed here is a registered one, so the fallback has no case.
            error: self.status.canonical_reason().unwrap_or("Error"),
            status_code: self.status,
        };

        (self.status, Json(body)).into_response()
    }
}

/// The JSON body returned for API errors.
#[derive(Debug, Serialize)]
struct ApiErrorResponse<'a> {
    message: &'a str,
    error: &'static str,
    #[serde(serialize_with = "code")]
    status_code: StatusCode,
}

fn custom(status: StatusCode, message: impl Into<Cow<'static, str>>) -> BoxedAppError {
    Box::new(CustomApiError {
        status,
        message: message.into(),
    })
}

/// What this deployment has never heard of.
///
/// The message is the caller's, which is what makes it useful and what makes it
/// worth being careful about: it is echoed from the request line, so it reaches
/// a log and a browser. Nothing else of ours goes into it.
pub(crate) fn not_found(message: impl Into<Cow<'static, str>>) -> BoxedAppError {
    custom(StatusCode::NOT_FOUND, message)
}

fn code<S: Serializer>(status: &StatusCode, out: S) -> Result<S::Ok, S::Error> {
    out.serialize_u16(status.as_u16())
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    async fn answered(error: BoxedAppError) -> (StatusCode, String) {
        let response = error.into_response();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();

        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn serialises_in_the_order_the_typescript_emits() {
        // Measured off the running service, and the order is load-bearing: the
        // Phase 2 gate compares bytes, and the DTO class declares these three
        // the other way round.
        let (status, body) = answered(not_found("Cannot GET /nope")).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            r#"{"message":"Cannot GET /nope","error":"Not Found","status_code":404}"#
        );
    }

    #[tokio::test]
    async fn the_reason_phrase_is_the_status_and_is_not_passed_in() {
        // The one thing the constructors no longer carry. A 400 built here has
        // never had "Bad Request" written next to it.
        let (status, body) =
            answered(custom(StatusCode::BAD_REQUEST, "asOf is in the future")).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            r#"{"message":"asOf is in the future","error":"Bad Request","status_code":400}"#
        );
    }

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
