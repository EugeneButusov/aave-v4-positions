//! The failures that answer with a JSON body, and what that body looks like.
//!
//! Separate from its parent for the reason
//! [crates.io](https://github.com/rust-lang/crates.io/blob/main/src/util/errors/json.rs)
//! separates it: the two halves change for different reasons. The wire shape
//! below moves when the contract the callers parse moves, which is a release
//! note; the trait beside it moves when this service grows a new way to fail,
//! which nobody outside can see.
//!
//! **The envelope is Nest's, deliberately.** `api-error.dto.ts` explains why it
//! is not a custom one: the default filter already produces this shape for every
//! `HttpException` in the application, "including the ones Nest raises itself for
//! an unknown route — inventing a different one would mean either catching those
//! too or publishing a contract with two error shapes in it". crates.io's own
//! envelope is `{"errors":[{"detail":…}]}`, which is the registry API's
//! convention and not ours to borrow — the module is the thing worth taking,
//! not the JSON in it.
//!
//! **The field order is measured**, and it is not the order the DTO class
//! declares: the running service emits `message`, then `error`, then the status.
//! A byte-comparing gate sees the difference, so the order is the contract even
//! though the third key's spelling is not.
//!
//! **That third key is `status_code` where the TypeScript says `statusCode`**,
//! the same deliberate deviation the probe surface makes and for the same
//! reason — this port reads as Rust rather than as a transliteration. This used
//! to be one of two such keys; it is now one instance of the rule
//! `docs/rust-migration.md` states for the whole wire, which the positions
//! payload's twenty-four made unavoidable. Callers parse error bodies, so it
//! goes in that rule's release note rather than passing unnoticed.

use std::fmt;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Serialize, Serializer};

use super::{AppError, BoxedAppError};

/// A failure the caller caused, carrying the status it deserves.
///
/// **`String` where crates.io takes `impl Into<Cow<'static, str>>`**, because
/// the borrowed half of that has no caller worth the type. This used to predict
/// that the first 400 would carry a constant and earn the `Cow` back. Six of the
/// route's refusals name the parameter and quote what was sent, so they are
/// formatted like every other message here; the two the cursor codec raises are
/// constants, and they allocate once on a path that is already answering an
/// error.
pub(crate) fn custom(status: StatusCode, message: String) -> BoxedAppError {
    Box::new(CustomApiError { status, message })
}

/// **Deliberately not a `std::error::Error`**, which is why `Display` is written
/// out rather than derived with `thiserror`: implementing it would overlap the
/// blanket impl in the parent and the two would no longer be distinguishable.
/// crates.io hand-writes the same `Display` for the same reason.
#[derive(Debug)]
struct CustomApiError {
    status: StatusCode,
    message: String,
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
            // reaching here is a registered one, so the fallback has no case.
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

fn code<S: Serializer>(status: &StatusCode, out: S) -> Result<S::Ok, S::Error> {
    out.serialize_u16(status.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors;
    use crate::test_support::answered;

    #[tokio::test]
    async fn serialises_in_the_order_the_typescript_emits() {
        // Measured off the running service, and the order is load-bearing: the
        // Phase 2 gate compares bytes, and the DTO class declares these three
        // the other way round.
        let (status, body) = answered(errors::not_found("Cannot GET /nope".to_owned())).await;

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
        let (status, body) = answered(custom(
            StatusCode::BAD_REQUEST,
            "asOf is in the future".to_owned(),
        ))
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            r#"{"message":"asOf is in the future","error":"Bad Request","status_code":400}"#
        );
    }
}
