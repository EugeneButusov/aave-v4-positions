//! What a process has to have decided before anything can be exported.

use tracing::level_filters::LevelFilter;

/// One parsed configuration, handed down rather than read.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Every signal is grouped by this. A binary refuses to boot without one.
    pub service: String,
    /// Where each signal is posted, paths included.
    pub endpoints: Endpoints,
    pub sampler: Sampling,
    /// Read by the two ratio samplers, ignored by the other four.
    pub ratio: f64,
    pub level: LevelFilter,
    pub pretty: bool,
    /// No providers and no exporters: the formatter alone.
    pub disabled: bool,
}

/// Where each signal is posted.
///
/// Complete addresses, not a base: `with_endpoint` takes them verbatim, and
/// `/v1/{signal}` is OTLP/HTTP's path rather than anything this crate decides.
/// Composing them from `OTEL_EXPORTER_OTLP_ENDPOINT` belongs to whoever maps
/// an environment onto a service — `bins/api/src/config.rs` for the API.
#[derive(Clone, Debug)]
pub struct Endpoints {
    pub traces: String,
    pub metrics: String,
    pub logs: String,
}

/// The six the specification spells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sampling {
    AlwaysOn,
    AlwaysOff,
    TraceIdRatio,
    ParentBasedAlwaysOn,
    ParentBasedAlwaysOff,
    ParentBasedTraceIdRatio,
}
