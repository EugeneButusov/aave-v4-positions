//! The failures that answer with a JSON body, and what that body looks like.
//!
//! Separate from its parent because the two change for different reasons: the
//! wire shape below moves when the contract moves, which is a release note.
//!
//! One envelope for every refusal, including the ones the framework raises for
//! an unknown route.
//!
//! Field order is part of the contract — `message`, then `error`, then the
//! status — as is the spelling of the third key (`docs/rust-migration.md`).

use std::fmt;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Serialize, Serializer};
use utoipa::ToSchema;

use super::ApiError;

/// A failure the caller caused, carrying the status it deserves.
pub(crate) fn refused(status: StatusCode, message: String) -> ApiError {
    ApiError::Refused(Box::new(Envelope::new(status, message)))
}

// Deliberately not a `std::error::Error`: it would overlap the blanket impl in
// the parent. The `///` below is the published schema description, so nothing
// about this file's internals belongs in it.
//
// `ApiErrorResponse` is the published component name and is contractual; the
// type is not a response, it is the body of one.
/// The JSON body returned for API errors.
#[derive(Debug, Serialize, ToSchema)]
#[schema(as = ApiErrorResponse)]
pub(crate) struct Envelope {
    message: String,
    error: &'static str,
    #[serde(serialize_with = "code")]
    #[schema(value_type = u16, example = 404)]
    status_code: StatusCode,
}

impl Envelope {
    fn new(status: StatusCode, message: String) -> Self {
        Self {
            message,
            // Derived rather than passed: the `error` key is the status's
            // reason phrase, and every status reaching here is registered.
            error: status.canonical_reason().unwrap_or("Error"),
            status_code: status,
        }
    }
}

impl fmt::Display for Envelope {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(out)
    }
}

impl IntoResponse for Envelope {
    fn into_response(self) -> Response {
        (self.status_code, Json(self)).into_response()
    }
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
        // A 400 built here has never had "Bad Request" written next to it.
        let (status, body) = answered(refused(
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
