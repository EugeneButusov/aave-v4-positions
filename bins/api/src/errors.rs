//! How this service represents a failure, and what a caller learns from one.
//!
//! Two shapes and no more: anything that went wrong on our side, which answers
//! a 500 that says nothing about itself, and something the caller got wrong,
//! which answers the envelope naming it. Any `std` error becomes the first by
//! `?`, so a handler needs no conversion to return a store read.
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

/// Everything a request can fail with.
#[derive(Debug)]
pub(crate) enum ApiError {
    /// Ours, and the caller learns only the status. What the error said goes
    /// to the log instead: this service answers for any wallet anyone asks
    /// about.
    Internal(Box<dyn Error + Send>),

    /// The caller's, said back to them. Boxed so a `Result` carrying this
    /// stays two words rather than six.
    Refused(Box<ApiErrorResponse>),
}

impl fmt::Display for ApiError {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Internal(error) => error.fmt(out),
            Self::Refused(refusal) => refusal.fmt(out),
        }
    }
}

/// Every `std` error becomes an [`ApiError::Internal`], so a handler can `?` a
/// store read. No overlap with `From<T> for T`, because this is not itself a
/// `std::error::Error`.
impl<E: Error + Send + 'static> From<E> for ApiError {
    fn from(error: E) -> Self {
        Self::Internal(Box::new(error))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::Internal(error) => {
                // The chain, not just the head: a store names its own stage and
                // leaves the cause to `source()`, so `%error` never says which
                // half failed.
                tracing::error!(
                    error = %ops::error_message_with_causes(&*error),
                    "a request failed"
                );

                (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response()
            }
            Self::Refused(refusal) => refusal.into_response(),
        }
    }
}

/// What this deployment has never heard of.
///
/// The message is echoed from the request line, so it reaches a log and a
/// browser. Nothing else of ours goes into it.
pub(crate) fn not_found(message: String) -> ApiError {
    json::refused(StatusCode::NOT_FOUND, message)
}

/// What the caller got wrong, said back to them. Without it a malformed
/// address answers a 500, telling a caller their mistake is the server's.
pub(crate) fn bad_request(message: String) -> ApiError {
    json::refused(StatusCode::BAD_REQUEST, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::answered;

    #[tokio::test]
    async fn a_std_error_is_a_500_that_tells_the_caller_nothing() {
        // Why the internal arm answers fixed text: this message names an
        // internal host, which forwarding `Display` would put on the wire.
        let failed = std::io::Error::other("connection refused: clickhouse.internal:8123");
        let (status, body) = answered(failed.into()).await;

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
                .block_on(answered(failed.into()))
        });
        let logged = written.read();

        assert!(logged.contains("the sync status read failed"), "{logged}");
        assert!(logged.contains("Connection refused"), "{logged}");
        assert_eq!(body, "Internal Server Error");
    }
}
