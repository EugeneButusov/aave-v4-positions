//! How a path this service does not serve is refused, and where the versioned
//! ones hang.
//!
//! Nothing this service does is named here — that is [`crate::app`]'s. What is
//! left is prefix arithmetic and two fallbacks.

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

/// Where the document hangs, given whatever a deployment wrote in
/// `API_DOCS_PATH`. Trimmed as in [`mount`].
///
/// Outside the prefix and unversioned, like the probes.
pub(crate) fn docs(path: &str) -> String {
    match path.trim_matches('/') {
        "" => String::new(),
        path => format!("/{path}"),
    }
}

/// Both fallbacks, over a router that is already finished.
///
/// Takes the router rather than being called on one, because
/// `method_not_allowed_fallback` only covers the `MethodRouter`s already
/// registered — a route added afterwards would answer axum's 405.
pub(crate) fn refuse_unmatched(router: Router) -> Router {
    router
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
}

/// What both fallbacks answer. The contract does not distinguish an unmatched
/// path from an unmatched method: `POST /health/live` is
/// `404 Cannot POST /health/live`, where axum's default is a 405.
///
/// The whole request target, not just the path, which `uri.path()` would drop.
async fn not_found(method: Method, uri: Uri) -> BoxedAppError {
    let target = uri
        .path_and_query()
        .map_or_else(|| uri.path(), axum::http::uri::PathAndQuery::as_str);

    errors::not_found(format!("Cannot {method} {target}"))
}

#[cfg(test)]
mod tests {
    //! Driven through `app::handler`, so a route moved out from under the
    //! fallbacks fails here.

    use axum::body::Body;
    use axum::http::{Request, StatusCode};

    use super::{docs, mount};

    use crate::test_support::{get, handler, postgres};

    /// A 404 for a path this service does not serve *and* for a method it does
    /// not serve on a path it does, echoing the whole request target.
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
    async fn an_unknown_path_answers_the_envelope_a_refusal_carries() {
        // Query string included: the obvious `uri.path()` would drop it.
        let (status, body) = refused("GET", "/nope?a=1&b=2").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            r#"{"message":"Cannot GET /nope?a=1&b=2","error":"Not Found","status_code":404}"#
        );
    }

    #[tokio::test]
    async fn the_method_fallback_reaches_a_nested_route_too() {
        // Applied to a finished router, so the versioned routes nested inside
        // are covered too. Applied before the nesting, this is a 405.
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

    #[tokio::test]
    async fn the_versioned_routes_are_not_reachable_without_the_prefix() {
        let (status, _) = refused(
            "GET",
            "/v1/chains/1/users/0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8/positions",
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn mounts_the_document_wherever_api_docs_path_says() {
        assert_eq!(docs("docs"), "/docs");
        assert_eq!(docs("/docs/"), "/docs", "however a deployment wrote it");
        assert_eq!(docs(""), "", "no path is not an empty segment");
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
        // Where the port talks axum out of its own 405.
        let (status, body) = refused("POST", "/health/live").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            r#"{"message":"Cannot POST /health/live","error":"Not Found","status_code":404}"#
        );
    }
}
