//! What a failure looks like on the wire.
//!
//! One type, one `IntoResponse`, constructors beside it — the structure
//! [crates.io](https://github.com/rust-lang/crates.io/blob/main/src/util/errors.rs) and
//! [Zed's collab server](https://github.com/zed-industries/zed/blob/main/crates/collab/src/lib.rs)
//! independently arrive at, and what replaces Nest's exception filter.
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
//! declares: the running service emits `message`, then `error`, then
//! `statusCode`. A byte-comparing gate sees the difference.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Serialize, Serializer};

#[derive(Debug, Serialize)]
pub(crate) struct ApiError {
    message: String,
    /// The status's reason phrase — `Not Found`, and `Bad Request` when
    /// validation arrives.
    ///
    /// **Not an `Option`, though Nest omits it on a plain 500.** Nothing here
    /// produces a 500 through this type — a panic answers with fixed text from
    /// the middleware — so a `skip_serializing_if` would be a rule with no case
    /// behind it, and serde attributes are not `dead_code`. It becomes optional
    /// the day something needs it to be.
    error: &'static str,
    #[serde(rename = "statusCode", serialize_with = "code")]
    status: StatusCode,
}

impl ApiError {
    /// What this deployment has never heard of.
    ///
    /// The message is the caller's, which is what makes it useful and what makes
    /// it worth being careful about: it is echoed from the request line, so it
    /// reaches a log and a browser. Nothing else of ours goes into it.
    pub(crate) fn not_found(message: String) -> Self {
        Self {
            message,
            error: "Not Found",
            status: StatusCode::NOT_FOUND,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self)).into_response()
    }
}

fn code<S: Serializer>(status: &StatusCode, out: S) -> Result<S::Ok, S::Error> {
    out.serialize_u16(status.as_u16())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(error: &ApiError) -> String {
        serde_json::to_string(error).unwrap()
    }

    #[test]
    fn serialises_in_the_order_the_typescript_emits() {
        // Measured off the running service, and the order is load-bearing: the
        // Phase 2 gate compares bytes, and the DTO class declares these three
        // the other way round.
        assert_eq!(
            json(&ApiError::not_found("Cannot GET /nope".to_owned())),
            r#"{"message":"Cannot GET /nope","error":"Not Found","statusCode":404}"#
        );
    }

    #[test]
    fn carries_the_status_as_a_number_not_a_reason_phrase() {
        let error = ApiError::not_found(String::new());

        assert!(
            json(&error).contains(r#""statusCode":404"#),
            "{}",
            json(&error)
        );
    }
}
