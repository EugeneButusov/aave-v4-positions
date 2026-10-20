//! What one request produced in spans and log lines.
//!
//! Each case gets a subscriber of its own, so they run in parallel without
//! reading each other's telemetry. Nothing here knows what this service does —
//! it takes a router and hands back what came out of it.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::Request;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use tower::ServiceExt;
use tracing_subscriber::layer::SubscriberExt as _;

/// Everything one request produced, through a subscriber of the case's own, so
/// cases run in parallel without reading each other's telemetry.
pub(crate) struct Observed {
    pub(crate) request_id: Option<String>,
    pub(crate) spans: Vec<SpanData>,

    /// The JSON lines the formatter wrote.
    pub(crate) logged: String,
}

/// Makes every callsite in this binary interesting, once.
///
/// `tracing` caches callsite interest from the *global* subscriber, which is
/// `NoSubscriber` here and answers "never". Without this, a case reads an empty
/// log and a correct response — a harness failure wearing the code's clothes.
pub(crate) fn interesting() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();

    ONCE.get_or_init(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(tracing::level_filters::LevelFilter::TRACE),
        );
    });
}

pub(crate) fn observed(router: Router, request: Request<Body>) -> Observed {
    interesting();

    // A global by design, and every case sets the same one.
    opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let written = Written::default();
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(written.clone()),
        )
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer(telemetry::SCOPE)));

    let request_id = tracing::subscriber::with_default(subscriber, || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                router
                    .oneshot(request)
                    .await
                    .unwrap()
                    .headers()
                    .get("x-request-id")
                    .map(|value| value.to_str().unwrap().to_owned())
            })
    });
    provider.force_flush().unwrap();

    Observed {
        request_id,
        spans: exporter.get_finished_spans().unwrap(),
        logged: written.read(),
    }
}

/// What a subscriber wrote, held where a case can read it back. One of these,
/// for both suites here.
#[derive(Clone, Default)]
pub(crate) struct Written(Arc<std::sync::Mutex<Vec<u8>>>);

impl Written {
    pub(crate) fn read(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl std::io::Write for Written {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Written {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}
