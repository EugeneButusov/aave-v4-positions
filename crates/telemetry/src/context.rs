//! A request's trace, on the way in and on the way out.

use opentelemetry::trace::TraceContextExt as _;
use opentelemetry_http::HeaderExtractor;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

/// Makes the span a continuation of the caller's trace, when they sent one.
///
/// Takes the span rather than the current one: parenting happens before it is
/// entered.
pub fn continue_from(span: &tracing::Span, headers: &http::HeaderMap) {
    let caller = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(headers))
    });

    // Refused when telemetry is off, and there is nothing to parent then.
    let _ = span.set_parent(caller);
}

/// The trace this span belongs to, or `None` when nothing is tracing.
#[must_use]
pub fn trace_id(span: &tracing::Span) -> Option<String> {
    let context = span.context();
    let span = context.span();
    let context = span.span_context();

    context.is_valid().then(|| context.trace_id().to_string())
}
