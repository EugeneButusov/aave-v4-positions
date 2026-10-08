//! What a process has to have decided before anything can be exported.

use tracing::level_filters::LevelFilter;

/// One parsed configuration, handed down rather than read.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Every signal is grouped by this. A binary refuses to boot without one.
    pub service: String,
    /// The collector's base address. The signal paths are this crate's to add.
    pub endpoint: String,
    pub sampler: Sampling,
    /// Read by the two ratio samplers, ignored by the other four.
    pub ratio: f64,
    pub level: LevelFilter,
    pub pretty: bool,
    /// No providers and no exporters: the formatter alone.
    pub disabled: bool,
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
