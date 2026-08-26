//! Every path this service answers on.
//!
//! Probes and the two fallbacks, for now. The versioned routes and the prefix
//! they sit under arrive with the positions endpoint; a prefix with nothing
//! beneath it is configuration that cannot be observably wrong.
//!
//! **The dependencies are named here, in a list, and that is the whole
//! registry.** `ops` owns the report and the paths; this owns which databases
//! this process answers for.

use axum::Router;
use axum::http::{Method, Uri};

use crate::app::AppState;
use crate::errors::ApiError;

pub(crate) fn build(state: AppState) -> Router {
    let (uptime, drain) = (state.uptime, state.drain.clone());

    ops::probe_router(uptime, drain, move || {
        let state = state.clone();
        async move {
            // Side by side rather than one after the other, as the TypeScript's
            // `Promise.all` does: neither answer depends on the other, and a
            // probe that serialises them reports the sum of two timeouts.
            // `join!` keeps the order of the results, which the wire contract
            // fixes.
            let (clickhouse, postgres) = tokio::join!(
                ops::check("clickhouse", clickhouse_client::ping(&state.clickhouse)),
                ops::check("postgres", ::postgres::ping(&state.postgres)),
            );
            vec![clickhouse, postgres]
        }
    })
    .fallback(not_found)
    // After the routes, because it sets a fallback on every `MethodRouter`
    // already registered — before them it would have none to set.
    .method_not_allowed_fallback(not_found)
}

/// Both fallbacks, and that is the finding rather than a shortcut.
///
/// Express does not distinguish an unmatched path from an unmatched *method*:
/// `POST /health/live` answers `404 Cannot POST /health/live`, not `405`.
/// Measured against both services, because axum's default is a 405 with an
/// `allow` header and an empty body — a divergence nothing here would have
/// noticed, since neither service's suite covers a route it does not serve.
///
/// **The whole request target, not just the path.** Also measured:
/// `GET /nope?a=1&b=2` comes back as `Cannot GET /nope?a=1&b=2`, because Express
/// builds the message from `req.originalUrl`. Reaching for `uri.path()` would
/// have been the obvious thing and would have been wrong.
async fn not_found(method: Method, uri: Uri) -> ApiError {
    let target = uri
        .path_and_query()
        .map_or_else(|| uri.path(), axum::http::uri::PathAndQuery::as_str);

    ApiError::not_found(format!("Cannot {method} {target}"))
}

#[cfg(test)]
mod tests {
    //! Against real servers, because the wiring is the only thing left to get
    //! wrong: `ops` already proves the report and the drain in isolation, and
    //! what these add is that the names, the order and the two `ping`s behind
    //! them are hooked up to the databases this process actually opens.

    use axum::body::Body;
    use axum::http::{Request, StatusCode};

    use crate::test_support::{get, handler, postgres, unreachable_postgres};

    /// Every literal below was measured against the running TypeScript service.
    /// It answers a 404 for a path it does not serve *and* for a method it does
    /// not serve on a path it does, and it echoes the whole request target.
    async fn refused(method: &str, uri: &str) -> (StatusCode, String) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();

        let (status, body, _) = get(handler(postgres()), request).await;
        (status, body)
    }

    #[tokio::test]
    async fn an_unknown_path_answers_the_envelope_the_typescript_does() {
        // Query string included, because `req.originalUrl` carries it — the
        // obvious `uri.path()` would drop it and the gate would say so.
        let (status, body) = refused("GET", "/nope?a=1&b=2").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            r#"{"message":"Cannot GET /nope?a=1&b=2","error":"Not Found","statusCode":404}"#
        );
    }

    #[tokio::test]
    async fn a_wrong_method_on_a_real_path_is_a_404_and_not_a_405() {
        // axum's default here is a 405 with an `allow` header and no body.
        // Express treats an unmatched method as an unmatched route, so this is
        // the one place the port has to talk axum out of being more correct
        // than the thing it replaces.
        let (status, body) = refused("POST", "/health/live").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            r#"{"message":"Cannot POST /health/live","error":"Not Found","statusCode":404}"#
        );
    }

    fn ready() -> Request<Body> {
        Request::builder()
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn reports_both_databases_up_against_the_real_servers() {
        let (status, body, _) = get(handler(postgres()), ready()).await;

        assert_eq!(status, StatusCode::OK);
        // Byte for byte what the TypeScript service answered when both were
        // reachable, measured rather than transcribed from its DTOs.
        assert_eq!(
            body,
            r#"{"status":"ok","checks":[{"name":"clickhouse","status":"up"},{"name":"postgres","status":"up"}]}"#
        );
    }

    #[tokio::test]
    async fn names_postgres_when_only_postgres_is_unreachable() {
        // Which is the half a single aggregated boolean would lose, and the
        // reason the report carries names at all.
        let (status, body, _) = get(handler(unreachable_postgres()), ready()).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.starts_with(r#"{"status":"degraded","#), "{body}");
        assert!(
            body.contains(r#"{"name":"clickhouse","status":"up"}"#),
            "{body}"
        );
        assert!(
            body.contains(r#"{"name":"postgres","status":"down","error":"#),
            "{body}"
        );
    }
}
