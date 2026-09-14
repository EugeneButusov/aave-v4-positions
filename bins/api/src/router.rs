//! How a path this service does not serve is refused, and where the versioned
//! ones hang.
//!
//! **Nothing this service does is named here** — that is [`crate::app`]'s, which
//! already holds the stores behind it. What is left is the mechanism: prefix
//! arithmetic and two fallbacks, neither of which moves when an endpoint is
//! added.

use axum::Router;
use axum::http::{Method, Uri};

use crate::errors::{self, BoxedAppError};

/// Where the versioned routes hang, given whatever a deployment wrote in
/// `API_GLOBAL_PREFIX`. Slashes are trimmed, and an empty prefix is one asking
/// for none — so `/v1` rather than `//v1`.
pub(crate) fn mount(prefix: &str) -> String {
    match prefix.trim_matches('/') {
        "" => "/v1".to_owned(),
        prefix => format!("/{prefix}/v1"),
    }
}

/// Both fallbacks, over a router that is already finished.
///
/// **It takes the router rather than being called on one**, which makes the
/// ordering unwrongable: `method_not_allowed_fallback` sets a fallback on every
/// `MethodRouter` already registered, so a route added afterwards answers axum's
/// 405 where the contract says 404. As an argument, there is nothing to add.
pub(crate) fn refuse_unmatched(router: Router) -> Router {
    router
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
}

/// What both fallbacks answer, and the reason there are two of them.
///
/// Express does not distinguish an unmatched path from an unmatched *method*:
/// `POST /health/live` answers `404 Cannot POST /health/live`, not `405`. Both
/// measured, because axum's default is a 405 with an `allow` header and an empty
/// body, and neither suite covers a route it does not serve.
///
/// **The whole request target, not just the path**: `GET /nope?a=1&b=2` comes
/// back verbatim, because Express builds the message from `req.originalUrl`.
async fn not_found(method: Method, uri: Uri) -> BoxedAppError {
    let target = uri
        .path_and_query()
        .map_or_else(|| uri.path(), axum::http::uri::PathAndQuery::as_str);

    errors::not_found(format!("Cannot {method} {target}"))
}

#[cfg(test)]
mod tests {
    //! Against real servers, because the wiring is all that is left to get
    //! wrong: `ops` proves the report and the shutdown in isolation. They drive
    //! `app::handler`, so a route moved out from under the fallbacks fails here
    //! rather than in review.

    use axum::body::Body;
    use axum::http::{Request, StatusCode};

    use super::mount;

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
            r#"{"message":"Cannot GET /nope?a=1&b=2","error":"Not Found","status_code":404}"#
        );
    }

    #[tokio::test]
    async fn the_method_fallback_reaches_a_nested_route_too() {
        // The property `refuse_unmatched` exists to keep: it is applied to a
        // finished router, so the versioned routes nested inside it are covered
        // as well. Applied before the nesting, this answers axum's 405.
        let (status, body) = refused(
            "POST",
            "/api/v1/chains/1/users/0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8/positions",
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(
            body.starts_with(r#"{"message":"Cannot POST /api/v1/chains/1"#),
            "{body}"
        );
    }

    #[test]
    fn mounts_the_versioned_routes_under_the_prefix_and_nowhere_else() {
        // `mount` on its own, which is the one piece of prefix arithmetic.
        assert_eq!(mount("api"), "/api/v1");
        assert_eq!(mount("/api/"), "/api/v1", "however a deployment wrote it");
        assert_eq!(mount(""), "/v1", "no prefix is not an empty segment");
    }

    #[tokio::test]
    async fn a_wrong_method_on_a_real_path_is_a_404_and_not_a_405() {
        // The one place the port talks axum out of being more correct than the
        // thing it replaces: its default here is a 405 with an `allow` header.
        let (status, body) = refused("POST", "/health/live").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            r#"{"message":"Cannot POST /health/live","error":"Not Found","status_code":404}"#
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
