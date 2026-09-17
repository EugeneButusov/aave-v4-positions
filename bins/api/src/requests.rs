//! One span, one line and one measurement per request.
//!
//! **Hand-written, because Rust has no auto-instrumentation.** The service this
//! ports from got a server span, a request log line and a latency histogram from
//! three packages it named and nothing it wrote. All three are here instead, in
//! one pass over the request, which is also what lets the route, the status and
//! the latency reach all three without being carried between layers.
//!
//! **Probe traffic is none of it.** `/health/live` and `/health/ready` are
//! answered every few seconds by compose, by a load balancer and by an
//! orchestrator; left in, they are the overwhelming majority of every trace list
//! and every log stream, and a latency graph of a process that answers nothing
//! else.

use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use opentelemetry::metrics::{Histogram, Meter};
use opentelemetry::{KeyValue, global};
use tracing::Instrument as _;
use tracing::field::Empty;

/// Duration of HTTP server requests, in seconds.
///
/// The name, the unit and the bucket boundaries are the specification's, not
/// ours: this is the series three panels of the dashboard are already written
/// against, and the service this replaces published it without writing a line.
const DURATION: &str = "http.server.request.duration";

/// The advisory boundaries the specification gives for that histogram. Without
/// them the SDK's default buckets top out where a slow page begins.
const BUCKETS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

/// Built once, with the meter provider already installed.
#[derive(Clone)]
pub(crate) struct Instruments {
    duration: Histogram<f64>,
}

impl Instruments {
    pub(crate) fn new() -> Self {
        Self::from(global::meter(telemetry::SCOPE))
    }

    /// **The whole inventory, in one place.** Everything this binary records is
    /// made here, which is what `names_every_instrument_this_binary_records`
    /// can then be an inventory of rather than a spot check.
    fn from(meter: Meter) -> Self {
        Self {
            duration: meter
                .f64_histogram(DURATION)
                .with_unit("s")
                .with_description("Duration of HTTP server requests")
                .with_boundaries(BUCKETS.to_vec())
                .build(),
        }
    }
}

/// Everything one request is observed as.
pub(crate) async fn observe(instruments: Instruments, request: Request, next: Next) -> Response {
    let (method, path) = (request.method().clone(), request.uri().path().to_owned());

    if path.starts_with(ops::HEALTH_PREFIX) {
        return next.run(request).await;
    }

    // The template, never the path. A route is a bounded set the service
    // declares; a path is whatever a stranger sent, so filling this in from the
    // URL would let anyone mint unbounded metric series by varying a 404.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned());
    let scheme = request.uri().scheme_str().unwrap_or("http").to_owned();

    let span = tracing::info_span!(
        "request",
        // What the exporter publishes the span as. The specification's shape is
        // method and route, which is why an unmatched request is the method
        // alone rather than a name carrying somebody's URL.
        otel.name = route.as_ref().map_or_else(
            || method.to_string(),
            |route| format!("{method} {route}"),
        ),
        otel.kind = "server",
        otel.status_code = Empty,
        http.request.method = %method,
        http.route = Empty,
        url.path = %path,
        url.scheme = %scheme,
        http.response.status_code = Empty,
        request_id = Empty,
        // Recorded onto the line as well as the span, which is the job
        // `instrumentation-pino` did for one dependency on the other side.
        trace_id = Empty,
    );
    if let Some(route) = route.as_deref() {
        span.record("http.route", route);
    }
    telemetry::continue_from(&span, request.headers());
    if let Some(id) = telemetry::trace_id(&span) {
        span.record("trace_id", id);
    }

    let started = Instant::now();
    let response = next.run(request).instrument(span.clone()).await;
    let elapsed = started.elapsed();
    let status = response.status();

    span.record("http.response.status_code", status.as_u16());
    // Read back off the response rather than recorded where it is made:
    // `SetRequestId` only calls its maker when the caller sent no header, so
    // this is the one place both branches pass through.
    if let Some(id) = response
        .headers()
        .get("x-request-id")
        .and_then(|id| id.to_str().ok())
    {
        span.record("request_id", id);
    }
    if status.is_server_error() {
        // 5xx only. A refusal is the caller's fault and a correct answer by this
        // service, so marking those failed would leave no signal for the ones
        // that are ours.
        span.record("otel.status_code", "Error");
    }

    let mut attributes = vec![
        KeyValue::new("http.request.method", method.to_string()),
        KeyValue::new("url.scheme", scheme),
        KeyValue::new("http.response.status_code", i64::from(status.as_u16())),
    ];
    if let Some(route) = route {
        attributes.push(KeyValue::new("http.route", route));
    }
    instruments
        .duration
        .record(elapsed.as_secs_f64(), &attributes);

    line(
        &span,
        status,
        u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
    );
    response
}

/// One line per answered request, at the level the status earns.
///
/// **Flat fields, not the nested `req`/`res` the other service logs.** What a
/// deployment reads is that each line is one parseable object; the names inside
/// it are this process's to choose, and `tracing` already spells them.
///
/// No header is logged at all, which is why nothing here redacts one.
fn line(span: &tracing::Span, status: StatusCode, latency_ms: u64) {
    // Inside the span, so the formatter puts the method, the route, the status
    // and the trace id on the line beside this. Only the latency is the event's
    // own, because only the latency is not a property of the request.
    let _entered = span.enter();

    if status.is_server_error() {
        tracing::error!(latency_ms, "request completed");
    } else if status.is_client_error() {
        tracing::warn!(latency_ms, "request completed");
    } else {
        tracing::info!(latency_ms, "request completed");
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use axum::Router;
    use axum::body::Body;
    use axum::http::Request as Incoming;
    use axum::routing::get;
    use opentelemetry::metrics::MeterProvider as _;
    use opentelemetry::trace::Status;
    use opentelemetry_sdk::metrics::data::AggregatedMetrics;
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
    use opentelemetry_sdk::trace::SpanData;
    use tower::ServiceExt as _;

    use super::*;
    use crate::test_support::observed;

    /// A trace parent a caller might have sent, and the trace it names.
    const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    const CALLERS_TRACE: &str = "4bf92f3577b34da6a3ce929d0e0e4736";

    /// Shaped like the router this binary serves: one parameterised route, one
    /// probe, one way to fail.
    fn router(instruments: Instruments) -> Router {
        Router::new()
            .route("/v1/positions/{user}", get(async || "[]"))
            .route("/health/live", get(async || "{}"))
            .route("/boom", get(async || StatusCode::INTERNAL_SERVER_ERROR))
            .layer(axum::middleware::from_fn(move |request, next| {
                observe(instruments.clone(), request, next)
            }))
    }

    fn request(path: &str) -> Incoming<Body> {
        Incoming::builder().uri(path).body(Body::empty()).unwrap()
    }

    /// Drives the given paths and hands back every measurement they produced.
    async fn measured(paths: &[&str]) -> (InMemoryMetricExporter, SdkMeterProvider) {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let instruments = Instruments::from(provider.meter(telemetry::SCOPE));

        for path in paths {
            router(instruments.clone())
                .oneshot(request(path))
                .await
                .unwrap();
        }
        provider.force_flush().unwrap();

        (exporter, provider)
    }

    /// Every instrument name that reached the exporter.
    fn instrument_names(exporter: &InMemoryMetricExporter) -> BTreeSet<String> {
        exporter
            .get_finished_metrics()
            .unwrap()
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .map(|metric| metric.name().to_string())
            .collect()
    }

    /// The attribute sets of every histogram point recorded, as `key=value`.
    fn points(exporter: &InMemoryMetricExporter) -> Vec<BTreeSet<String>> {
        exporter
            .get_finished_metrics()
            .unwrap()
            .iter()
            .flat_map(|resource| resource.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .filter(|metric| metric.name() == DURATION)
            .filter_map(|metric| match metric.data() {
                AggregatedMetrics::F64(data) => Some(data),
                _ => None,
            })
            .flat_map(|data| match data {
                opentelemetry_sdk::metrics::data::MetricData::Histogram(histogram) => {
                    histogram.data_points().collect::<Vec<_>>()
                }
                _ => Vec::new(),
            })
            .map(|point| {
                point
                    .attributes()
                    .map(|attribute| format!("{}={}", attribute.key, attribute.value.as_str()))
                    .collect()
            })
            .collect()
    }

    #[tokio::test]
    async fn names_every_instrument_this_binary_records() {
        // The whole set, not a lookup of one name. An instrument that arrives
        // without a dashboard panel, or leaves with one still pointed at it,
        // fails here rather than showing up as a flat line nobody explains.
        let (exporter, _provider) = measured(&["/v1/positions/0xabc", "/boom"]).await;

        assert_eq!(
            instrument_names(&exporter),
            BTreeSet::from(["http.server.request.duration".to_owned()])
        );
    }

    #[tokio::test]
    async fn the_route_is_the_template_and_never_the_path() {
        // The cardinality rule. A route is a bounded set this service declares;
        // a path is whatever a stranger sent, so recording the second would let
        // anyone mint unbounded series by varying a URL.
        let (exporter, _provider) = measured(&["/v1/positions/0xdeadbeef"]).await;
        let points = points(&exporter);

        assert_eq!(points.len(), 1, "{points:?}");
        assert!(
            points[0].contains("http.route=/v1/positions/{user}"),
            "{points:?}"
        );
        assert!(
            !points.iter().any(|point| point
                .iter()
                .any(|attribute| attribute.contains("0xdeadbeef"))),
            "the path reached a metric attribute: {points:?}"
        );
    }

    #[tokio::test]
    async fn an_unmatched_path_reports_no_route_at_all() {
        // The same rule from the other side: an absent attribute rather than
        // one holding the URL that missed.
        let (exporter, _provider) = measured(&["/not-a-route"]).await;
        let points = points(&exporter);

        assert_eq!(points.len(), 1, "{points:?}");
        assert!(
            !points[0]
                .iter()
                .any(|attribute| attribute.starts_with("http.route=")),
            "{points:?}"
        );
    }

    #[tokio::test]
    async fn every_answered_request_is_timed_once() {
        let (exporter, _provider) =
            measured(&["/v1/positions/0xa", "/v1/positions/0xb", "/boom"]).await;
        let points = points(&exporter);

        // Two series: the route answered twice, and the refusal once.
        assert_eq!(points.len(), 2, "{points:?}");
        assert!(
            points
                .iter()
                .any(|point| point.contains("http.response.status_code=500")),
            "{points:?}"
        );
    }

    /// One attribute of a span, as the string it was recorded as.
    fn attribute(span: &SpanData, key: &str) -> Option<String> {
        span.attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == key)
            .map(|attribute| attribute.value.as_str().into_owned())
    }

    #[tokio::test]
    async fn probes_are_not_timed() {
        // They arrive every few seconds from compose, a load balancer and an
        // orchestrator. Left in, the latency graph is a graph of answering them.
        let (exporter, _provider) = measured(&["/health/live"]).await;

        assert!(instrument_names(&exporter).is_empty());
    }

    #[test]
    fn probes_are_not_traced_either() {
        let spans = observed(router(Instruments::new()), request("/health/live")).spans;

        assert!(spans.is_empty(), "{spans:?}");
    }

    #[test]
    fn names_the_span_for_the_route_and_reports_what_the_caller_got() {
        let spans = observed(router(Instruments::new()), request("/v1/positions/0xabc")).spans;

        assert_eq!(spans.len(), 1, "{spans:?}");
        assert_eq!(spans[0].name, "GET /v1/positions/{user}");
        assert_eq!(
            attribute(&spans[0], "http.response.status_code").as_deref(),
            Some("200")
        );
        assert_eq!(
            attribute(&spans[0], "http.route").as_deref(),
            Some("/v1/positions/{user}")
        );
        assert_eq!(spans[0].status, Status::Unset);
    }

    #[test]
    fn an_unmatched_request_is_named_by_its_method_alone() {
        // The other half of the cardinality rule: a span name is a low-cardinality
        // label too, so it cannot be built from a URL that missed.
        let spans = observed(router(Instruments::new()), request("/not-a-route")).spans;

        assert_eq!(spans.len(), 1, "{spans:?}");
        assert_eq!(spans[0].name, "GET");
        assert_eq!(attribute(&spans[0], "http.route"), None);
    }

    #[test]
    fn only_a_fault_of_ours_marks_the_span_failed() {
        // A refusal is the caller's fault and a correct answer by this service.
        // Marking those failed leaves no signal for the ones that are ours.
        let refused = observed(router(Instruments::new()), request("/not-a-route")).spans;
        let broken = observed(router(Instruments::new()), request("/boom")).spans;

        assert_eq!(refused[0].status, Status::Unset);
        assert_eq!(
            broken[0].status,
            Status::Error {
                description: String::new().into()
            }
        );
    }

    #[test]
    fn every_line_a_request_emits_carries_its_trace_id() {
        // The job `instrumentation-pino` did for one dependency on the other
        // side: a log line and a span that name the same trace, so one leads to
        // the other. Without it the two streams are unrelated.
        let observed = observed(router(Instruments::new()), request("/v1/positions/0xabc"));
        let trace = observed.spans[0].span_context.trace_id().to_string();

        assert!(
            observed.logged.contains("request completed"),
            "{}",
            observed.logged
        );
        assert!(
            observed
                .logged
                .contains(&format!(r#""trace_id":"{trace}""#)),
            "{}",
            observed.logged
        );
    }

    #[test]
    fn a_line_is_warned_for_a_refusal_and_errored_for_a_fault() {
        // `pino-http`'s mapping, which is what a log-level alert is written
        // against: a 404 is somebody else's mistake and a 500 is ours.
        let refused = observed(router(Instruments::new()), request("/not-a-route")).logged;
        let broken = observed(router(Instruments::new()), request("/boom")).logged;
        let served = observed(router(Instruments::new()), request("/v1/positions/0xa")).logged;

        assert!(refused.contains(r#""level":"WARN""#), "{refused}");
        assert!(broken.contains(r#""level":"ERROR""#), "{broken}");
        assert!(served.contains(r#""level":"INFO""#), "{served}");
    }

    #[test]
    fn a_probe_is_not_logged_at_all() {
        // They arrive every few seconds from three places at once. Logged, they
        // bury everything a stream is read for.
        let logged = observed(router(Instruments::new()), request("/health/live")).logged;

        assert!(logged.is_empty(), "{logged}");
    }

    #[test]
    fn continues_the_trace_the_caller_sent() {
        // Without this the service starts a trace of its own and the caller's
        // span tree stops at the edge of this process.
        let carried = Incoming::builder()
            .uri("/v1/positions/0xabc")
            .header("traceparent", TRACEPARENT)
            .body(Body::empty())
            .unwrap();

        let spans = observed(router(Instruments::new()), carried).spans;

        assert_eq!(spans[0].span_context.trace_id().to_string(), CALLERS_TRACE);
        assert!(spans[0].parent_span_id.to_string() != "0000000000000000");
    }
}
