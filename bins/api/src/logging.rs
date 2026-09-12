//! One JSON object per line, on stdout.
//!
//! `tracing-subscriber`'s own formatter rather than anything of ours: what a
//! deployment reads is that each line parses, and the fields a line carries are
//! `tracing`'s to name.
//!
//! **No `service` tag yet, and no request logging.** Both belong to the next
//! two increments and neither is a gap here. The tag's real home is OTLP's
//! `service.name` resource attribute, which arrives with `telemetry`; and the
//! TypeScript excludes `/health` from request logging, so a process serving
//! only probes emits exactly what its predecessor does for the same traffic —
//! nothing. `tower-http`'s `TraceLayer` lands with the first route worth
//! tracing, where the exclusion has something to exclude.

use tracing::level_filters::LevelFilter;

/// Installs the process-wide subscriber. Call once, as early as the level is
/// known.
pub(crate) fn init(level: LevelFilter, pretty: bool) {
    let builder = tracing_subscriber::fmt().with_max_level(level);

    if pretty {
        // Local development only. Anywhere else this must stay off so each line
        // remains a single parseable object.
        builder.pretty().init();
    } else {
        builder.json().init();
    }
}
