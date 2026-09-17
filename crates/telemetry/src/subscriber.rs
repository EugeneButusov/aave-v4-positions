//! One subscriber, and everything that hangs off it.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

use crate::provider::{self, Providers};
use crate::settings::Settings;

/// The instrumentation scope every span and instrument this workspace records
/// is attributed to.
pub const SCOPE: &str = "aave-v4-positions";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("telemetry needs a Tokio runtime to export from")]
    Runtime,
    #[error("building an OTLP exporter: {0}")]
    Exporter(#[from] opentelemetry_otlp::ExporterBuildError),
    #[error("installing the subscriber: {0}")]
    Subscriber(#[from] tracing_subscriber::util::TryInitError),
}

/// What the process holds until it is ready to stop exporting.
pub struct Telemetry(Option<Providers>);

impl Telemetry {
    /// Flushes what is batched and stops the three exporters.
    ///
    /// **Blocking, and deliberately called last.** Each provider waits for a
    /// thread that is posting the final batch through the runtime, so this must
    /// not run on the last thread able to serve that post — `spawn_blocking` is
    /// what the caller uses.
    pub fn shutdown(self) {
        let Some(providers) = self.0 else {
            return;
        };

        // Reported rather than propagated: the process is already stopping, and
        // a failure to export the last batch is not a reason to change how it
        // exits.
        if let Err(error) = providers.traces.shutdown() {
            tracing::warn!(%error, "the trace exporter did not stop cleanly");
        }
        if let Err(error) = providers.meters.shutdown() {
            tracing::warn!(%error, "the metric exporter did not stop cleanly");
        }
        if let Err(error) = providers.loggers.shutdown() {
            tracing::warn!(%error, "the log exporter did not stop cleanly");
        }
    }
}

/// Installs the process-wide subscriber. Call once, and before anything that
/// might want to log.
///
/// # Errors
///
/// [`Error`] when there is no runtime to export from, when an exporter cannot be
/// built, or when a subscriber is already installed.
pub fn init(settings: Settings) -> Result<Telemetry, Error> {
    let format = formatter(&settings);

    if settings.disabled {
        tracing_subscriber::registry()
            .with(settings.level)
            .with(format)
            .try_init()?;
        return Ok(Telemetry(None));
    }

    let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::Runtime)?;
    let providers = provider::build(&settings, runtime)?;

    opentelemetry::global::set_tracer_provider(providers.traces.clone());
    opentelemetry::global::set_meter_provider(providers.meters.clone());
    // W3C in and out. Inbound it is what continues a caller's trace; outbound it
    // is what the ClickHouse driver reads to put `traceparent` on its own
    // request, which is why this is a global rather than a field.
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    tracing_subscriber::registry()
        .with(settings.level)
        .with(format)
        .with(tracing_opentelemetry::layer().with_tracer(providers.traces.tracer(SCOPE)))
        .with(
            OpenTelemetryTracingBridge::new(&providers.loggers)
                .with_filter(tracing_subscriber::filter::filter_fn(ours)),
        )
        .try_init()?;

    Ok(Telemetry(Some(providers)))
}

/// **The exporters' own diagnostics do not become log records.** They are
/// `tracing` events like any other, and exporting them produces more of them:
/// one batch posted is several lines, which is another batch. stdout still
/// carries them, which is where a broken exporter is meant to be read.
fn ours(metadata: &tracing::Metadata<'_>) -> bool {
    !metadata.target().starts_with("opentelemetry")
}

/// One JSON object per line, on stdout.
///
/// `tracing-subscriber`'s own formatter rather than anything of ours: what a
/// deployment reads is that each line parses, and the fields a line carries are
/// `tracing`'s to name.
fn formatter<S>(settings: &Settings) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    if settings.pretty {
        // Local development only. Anywhere else this must stay off so each line
        // remains a single parseable object.
        Box::new(tracing_subscriber::fmt::layer().pretty())
    } else {
        Box::new(tracing_subscriber::fmt::layer().json())
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use tracing::level_filters::LevelFilter;

    use super::*;
    use crate::settings::Sampling;

    fn settings(disabled: bool) -> Settings {
        Settings {
            service: "api".to_owned(),
            endpoint: "http://localhost:4318".to_owned(),
            sampler: Sampling::ParentBasedAlwaysOn,
            ratio: 1.0,
            level: LevelFilter::INFO,
            pretty: false,
            disabled,
        }
    }

    /// Everything a filtered layer let through, as the bytes it wrote.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Captured {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.lock().map_or(Ok(0), |mut held| {
                held.extend_from_slice(buffer);
                Ok(buffer.len())
            })
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Emits both an exporter's line and a service's, through the same filter
    /// the log bridge carries, and returns what survived it.
    fn through_the_filter() -> String {
        let captured = Captured::default();
        let layer = tracing_subscriber::fmt::layer()
            .with_writer(captured.clone())
            .with_filter(tracing_subscriber::filter::filter_fn(ours));

        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            tracing::info!(target: "opentelemetry_sdk", "BatchSpanProcessor.ExportingDueToFlush");
            tracing::info!(target: "opentelemetry-otlp", "HttpTracesClient.ExportSucceeded");
            tracing::info!(target: "api::positions::list", "request completed");
        });

        let held = captured.0.lock().expect("nothing else holds this");
        String::from_utf8_lossy(&held).into_owned()
    }

    #[test]
    fn the_exporters_own_diagnostics_are_not_exported() {
        // Each posted batch logs several lines, and exporting those is another
        // batch. stdout still carries them, which is where a broken exporter is
        // meant to be read.
        let survived = through_the_filter();

        assert!(survived.contains("request completed"), "{survived}");
        assert!(!survived.contains("ExportingDueToFlush"), "{survived}");
        assert!(!survived.contains("ExportSucceeded"), "{survived}");
    }

    #[test]
    fn a_process_with_no_runtime_is_told_so_rather_than_panicking() {
        // The exporters post through the binary's runtime. Reaching for one
        // that is not there is a configuration error at boot, not a panic on
        // the first batch half a minute later.
        assert!(matches!(init(settings(false)), Err(Error::Runtime)));
    }

    #[test]
    fn disabled_asks_for_no_runtime_and_holds_no_exporter() {
        // Not an async test, deliberately: the disabled path must reach the
        // formatter without ever looking for a runtime to export from. It is
        // also the one case here that installs the process-wide subscriber.
        let telemetry = init(settings(true)).expect("the formatter needs nothing");

        assert!(telemetry.0.is_none());
        telemetry.shutdown();
    }
}
