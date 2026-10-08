//! One subscriber, and everything that hangs off it.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;

use crate::provider::{self, Providers};
use crate::settings::Settings;

/// The instrumentation scope everything here is attributed to.
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
    /// **Blocking.** Each provider waits on a thread posting its final batch
    /// through the runtime, so a caller runs this off a worker.
    pub fn shutdown(self) {
        let Some(providers) = self.0 else {
            return;
        };

        // The process is already stopping; a lost last batch changes nothing.
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
    // A global rather than a field because the ClickHouse driver reads it to
    // put `traceparent` on its own requests.
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

/// Keeps the exporters' own diagnostics out of the log bridge: exporting them
/// produces more of them, and one posted batch is several lines. stdout still
/// carries them.
fn ours(metadata: &tracing::Metadata<'_>) -> bool {
    !metadata.target().starts_with("opentelemetry")
}

/// One JSON object per line, on stdout.
fn formatter<S>(settings: &Settings) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    if settings.pretty {
        // Local only: anywhere else a line has to stay one parseable object.
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

    /// What survives the filter the log bridge carries.
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
        let survived = through_the_filter();

        assert!(survived.contains("request completed"), "{survived}");
        assert!(!survived.contains("ExportingDueToFlush"), "{survived}");
        assert!(!survived.contains("ExportSucceeded"), "{survived}");
    }

    #[test]
    fn a_process_with_no_runtime_is_told_so_rather_than_panicking() {
        // A configuration error at boot, not a panic on the first batch.
        assert!(matches!(init(settings(false)), Err(Error::Runtime)));
    }

    #[test]
    fn disabled_asks_for_no_runtime_and_holds_no_exporter() {
        // Not async, deliberately: the disabled path must never look for a
        // runtime. Also the one case here that installs a global subscriber.
        let telemetry = init(settings(true)).expect("the formatter needs nothing");

        assert!(telemetry.0.is_none());
        telemetry.shutdown();
    }
}
