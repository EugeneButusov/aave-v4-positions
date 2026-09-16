//! Where the contract is published, and in what.
//!
//! Three addresses, the same three the service this replaces serves: the UI at
//! `API_DOCS_PATH`, and the document itself as JSON and as YAML beneath it.
//! **Always served**, with no flag to turn it off — a contract absent from the
//! environment people actually call is not much of a contract.
//!
//! The UI's assets are vendored into the binary rather than fetched. Without
//! that, `utoipa-swagger-ui`'s build script downloads them at compile time,
//! which is not something the release image can do.

use std::sync::Arc;

use axum::Router;
use axum::http::header;
use axum::routing::get;
use utoipa::openapi::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::errors::BoxedAppError;

/// The UI, the JSON and the YAML, under whatever `mount` [`crate::router::docs`]
/// worked out. An empty `mount` puts the UI at the root.
pub(crate) fn routes(mount: &str, api: OpenApi) -> Router {
    let ui = if mount.is_empty() { "/" } else { mount };
    let json = format!("{mount}/openapi.json");
    let yaml = format!("{mount}/openapi.yaml");

    // The UI serves the JSON itself, from the same value, so the page and the
    // document it renders cannot be two documents.
    let swagger = SwaggerUi::new(ui.to_owned()).url(json, api.clone());

    let api = Arc::new(api);
    Router::new().merge(swagger).route(
        &yaml,
        get(move || {
            let api = Arc::clone(&api);
            async move {
                api.to_yaml()
                    .map(|yaml| ([(header::CONTENT_TYPE, "application/yaml")], yaml))
                    .map_err(BoxedAppError::from)
            }
        }),
    )
}

#[cfg(test)]
mod tests {
    //! Read back through the router, never rebuilt. A document a test assembles
    //! for itself is a second definition, and the thing most worth catching is
    //! the two disagreeing.

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;

    use crate::test_support::{Stores, get, postgres};

    async fn fetch(path: &str) -> (StatusCode, String) {
        let request = Request::builder()
            .uri(path)
            .body(Body::empty())
            .expect("a well-formed request");

        let (status, body, _) = get(Stores::default().handler(), request).await;
        (status, body)
    }

    async fn document() -> Value {
        let (status, body) = fetch("/docs/openapi.json").await;

        assert_eq!(status, StatusCode::OK);
        serde_json::from_str(&body).expect("the document is JSON")
    }

    /// The one operation, whatever else the document grows.
    fn positions(document: &Value) -> &Value {
        &document["paths"]["/api/v1/chains/{chain_id}/users/{user}/positions"]["get"]
    }

    fn property<'a>(document: &'a Value, schema: &str, field: &str) -> &'a Value {
        &document["components"]["schemas"][schema]["properties"][field]
    }

    /// `type` is one name or a list of them in OpenAPI 3.1, and a nullable field
    /// is spelled as the list. Both answer here as a list.
    fn types(schema: &Value) -> Vec<&str> {
        match &schema["type"] {
            Value::String(one) => vec![one.as_str()],
            Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
            other => panic!("no type on {other}"),
        }
    }

    fn required(document: &Value, schema: &str) -> Vec<String> {
        document["components"]["schemas"][schema]["required"]
            .as_array()
            .expect("a required list")
            .iter()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect()
    }

    #[tokio::test]
    async fn publishes_the_contract_this_build_serves() {
        // The guard. Everything below names one rule and would survive the rest
        // of the document changing; this is what notices that it did.
        insta::assert_json_snapshot!(document().await);
    }

    #[tokio::test]
    async fn gives_every_operation_a_typed_success_response() {
        // A route registered without a response body still routes and still
        // appears, but with nothing describing what it returns — and the
        // schemas it would have pulled in disappear with it, because a
        // component is only reachable through a response.
        let document = document().await;

        let mut missing = Vec::new();
        for (path, item) in document["paths"].as_object().expect("paths") {
            for (method, operation) in item.as_object().expect("an operation") {
                let typed = operation["responses"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(status, _)| status.starts_with('2'))
                    .any(|(_, response)| {
                        !response["content"]["application/json"]["schema"].is_null()
                    });

                if !typed {
                    missing.push(format!("{} {path}", method.to_uppercase()));
                }
            }
        }

        assert_eq!(missing, Vec::<String>::new());
    }

    #[tokio::test]
    async fn documents_exactly_the_path_this_service_versions() {
        // One, and the two routes serving the document are not among them: a
        // contract describing its own address is circular. The probes are
        // absent by the decision `crate::openapi` records.
        let document = document().await;

        let paths: Vec<_> = document["paths"]
            .as_object()
            .expect("paths")
            .keys()
            .map(String::as_str)
            .collect();

        assert_eq!(paths, ["/api/v1/chains/{chain_id}/users/{user}/positions"]);
    }

    #[tokio::test]
    async fn documents_the_path_as_deployed_rather_than_as_written() {
        // The route is declared as `/chains/{chain_id}/...` and the prefix is
        // the deployment's. `OpenApiRouter::nest` composes the two, so the
        // document cannot describe an address the router does not serve.
        let mut app = Stores::default().app(postgres());
        app.prefix = "gateway".to_owned();

        let request = Request::builder()
            .uri("/docs/openapi.json")
            .body(Body::empty())
            .expect("a well-formed request");

        let (_, body, _) = get(crate::app::handler(app), request).await;
        let document: Value = serde_json::from_str(&body).expect("the document is JSON");

        let paths: Vec<_> = document["paths"]
            .as_object()
            .expect("paths")
            .keys()
            .map(String::as_str)
            .collect();

        assert_eq!(
            paths,
            ["/gateway/v1/chains/{chain_id}/users/{user}/positions"]
        );
    }

    #[tokio::test]
    async fn types_every_share_and_amount_as_a_string() {
        // §7.5: a share balance has more significant digits than a double keeps,
        // so a schema calling one a number is an invitation to lose the tail.
        let document = document().await;

        for field in [
            "supplied_shares",
            "drawn_shares",
            "premium_shares",
            "premium_offset_ray",
            "net_supplied_amount",
            "net_borrowed_amount",
            "reserve_id",
        ] {
            let types = types(property(&document, "Item", field));
            assert!(types.contains(&"string"), "{field} is {types:?}");
        }

        for field in [
            "supplied_amount",
            "drawn_debt",
            "premium_debt",
            "total_debt",
            "drawn_index",
            "price_usd",
            "supplied_amount_usd",
            "total_debt_usd",
        ] {
            let types = types(property(&document, "Worth", field));
            assert!(types.contains(&"string"), "{field} is {types:?}");
        }
    }

    #[tokio::test]
    async fn marks_the_scale_dependent_fields_nullable_and_the_ray_not() {
        // The distinction the field docs draw: a share needs the asset's
        // decimals to render and a ray does not, so one survives an unresolved
        // reserve and the others cannot.
        let document = document().await;

        for field in [
            "supplied_shares",
            "drawn_shares",
            "premium_shares",
            "net_supplied_amount",
            "net_borrowed_amount",
        ] {
            assert_eq!(
                types(property(&document, "Item", field)),
                ["string", "null"],
                "{field}"
            );
        }

        assert_eq!(
            types(property(&document, "Item", "premium_offset_ray")),
            ["string"],
            "a ray's scale is the protocol's fixed 27"
        );
    }

    #[tokio::test]
    async fn marks_a_nullable_field_required_because_it_is_still_sent() {
        // utoipa reads an `Option` as optional; serde emits it as `null`. The
        // second is what a caller parses, so every one of them is listed.
        let document = document().await;

        for (schema, field) in [
            ("Page", "pricing"),
            ("Page", "next_cursor"),
            ("Item", "asset"),
            ("Item", "value"),
            ("Item", "supplied_shares"),
            ("Asset", "symbol"),
            ("Asset", "name"),
            ("Worth", "price_usd"),
            ("Worth", "total_debt_usd"),
        ] {
            assert!(
                required(&document, schema).iter().any(|name| name == field),
                "{schema}.{field}"
            );
        }
    }

    #[tokio::test]
    async fn types_all_three_clocks_the_same_way() {
        let document = document().await;

        for (schema, field) in [
            ("Page", "valued_at"),
            ("Progress", "updated_at"),
            ("Pricing", "updated_at"),
        ] {
            let clock = property(&document, schema, field);

            assert_eq!(types(clock), ["string"], "{schema}.{field}");
            assert_eq!(clock["format"], "date-time", "{schema}.{field}");
        }
    }

    #[tokio::test]
    async fn describes_the_refusals_and_not_only_the_page() {
        // A contract that documents the happy path alone leaves a caller to
        // discover the two refusals by causing them.
        let document = document().await;

        let statuses: Vec<_> = positions(&document)["responses"]
            .as_object()
            .expect("responses")
            .keys()
            .map(String::as_str)
            .collect();

        assert_eq!(statuses, ["200", "400", "404"]);

        for status in ["400", "404"] {
            assert_eq!(
                positions(&document)["responses"][status]["content"]["application/json"]["schema"]
                    ["$ref"],
                "#/components/schemas/ApiErrorResponse",
                "{status}"
            );
        }
    }

    #[tokio::test]
    async fn serves_the_ui_and_both_spellings_of_the_document() {
        let (status, body) = fetch("/docs/").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.contains("swagger"), "the UI is not html: {body:.120}");

        let (status, body) = fetch("/docs/openapi.yaml").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.starts_with("openapi: 3.1.0\n"), "{body:.60}");
    }

    #[tokio::test]
    async fn a_wrong_method_on_the_document_is_a_404_and_not_a_405() {
        // The property `router::refuse_unmatched` exists to keep, over routes
        // merged in after the versioned ones.
        let request = Request::builder()
            .method("POST")
            .uri("/docs/openapi.json")
            .body(Body::empty())
            .expect("a well-formed request");

        let (status, body, _) = get(Stores::default().handler(), request).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            r#"{"message":"Cannot POST /docs/openapi.json","error":"Not Found","status_code":404}"#
        );
    }
}
