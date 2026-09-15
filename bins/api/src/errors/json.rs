//! The failures that answer with a JSON body, and what that body looks like.
//!
//! Separate from its parent because the two halves change for different
//! reasons: the wire shape below moves when the contract callers parse moves,
//! which is a release note, and the trait beside it moves when this service
//! grows a new way to fail, which nobody outside can see.
//!
//! **One envelope for every refusal**, including the ones the framework raises
//! for an unknown route — a second shape would mean either catching those too
//! or publishing a contract with two of them.
//!
//! **Field order is part of the contract**: `message`, then `error`, then the
//! status. So is the spelling of the third key, which
//! `docs/rust-migration.md` covers under the rule it states for the whole wire.

use std::fmt;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Serialize, Serializer};

use super::{AppError, BoxedAppError};

/// A failure the caller caused, carrying the status it deserves.
///
/// `String` rather than `Cow`: six of the route's refusals quote what was sent,
/// and the cursor codec's two constants allocate once on a path that is already
/// answering an error.
pub(crate) fn custom(status: StatusCode, message: String) -> BoxedAppError {
    Box::new(CustomApiError { status, message })
}

/// **Deliberately not a `std::error::Error`**, which is why `Display` is written
/// out rather than derived with `thiserror`: implementing it would overlap the
/// blanket impl in the parent and the two would no longer be distinguishable.
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
            // Derived rather than passed: the `error` key is the status's
            // reason phrase and nothing else, and every status reaching here is
            // a registered one, so the fallback has no case.
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
    async fn serialises_in_the_order_the_contract_fixes() {
        // The order is load-bearing: a comparator reads it.
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
