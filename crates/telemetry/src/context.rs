//! A request's trace, on the way in and on the way out.
//!
//! **Here rather than in a binary**, because both halves are the propagator's
//! and the propagator is a global this crate sets. A caller names a span and a
//! set of headers; nothing about OpenTelemetry's context API leaks past this.

use opentelemetry::trace::TraceContextExt as _;
use opentelemetry_http::HeaderExtractor;
use tracing_opentelemetry::OpenTelemetrySpanExt as _;

/// Makes the span a continuation of the caller's trace, when they sent one.
///
/// Takes the span rather than reading the current one: a span is parented
/// before it is entered, and by the time it is current it is too late.
pub fn continue_from(span: &tracing::Span, headers: &http::HeaderMap) {
    let caller = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderExtractor(headers))
    });

    // Refused when the span carries no OpenTelemetry data, which is every span
    // in a process with telemetry switched off. There is nothing to parent and
    // nothing to report.
    let _ = span.set_parent(caller);
}

/// The trace this span belongs to, or `None` when nothing is tracing.
///
/// `None` rather than an error when telemetry is off: everything that asks has
/// to keep working either way, and it does, because the layer that would answer
/// is simply not installed.
#[must_use]
pub fn trace_id(span: &tracing::Span) -> Option<String> {
    let context = span.context();
    let span = context.span();
    let context = span.span_context();

    context.is_valid().then(|| context.trace_id().to_string())
}
