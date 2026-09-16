//! The three providers, and the one client all of them post through.

use std::sync::Arc;
use std::time::Duration;

use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response};
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_semantic_conventions::resource::SERVICE_NAME;

use crate::settings::{Sampling, Settings};

/// The SDK's default is 60 seconds. A gauge sampled once a minute cannot show a
/// backfill catching up, which is the thing these metrics exist for.
const EXPORT_INTERVAL: Duration = Duration::from_secs(15);

/// One posted batch. Long, because the alternative to waiting is losing the
/// batch, and short enough that a wedged collector cannot hold the drain open.
const EXPORT_TIMEOUT: Duration = Duration::from_secs(10);

/// Posts a batch, from a thread that has no runtime of its own.
///
/// The SDK's batch processors run on plain OS threads and drive the export with
/// `futures_executor::block_on`. Hyper's timers and sockets need a Tokio
/// reactor, and there is none on that thread — measured, it panics with "there
/// is no reactor running" on the first export. So the work is spawned back onto
/// the runtime the binary already has, and only the waiting happens on the batch
/// thread.
#[derive(Debug)]
struct Client {
    inner: Arc<opentelemetry_http::hyper::HyperClient>,
    runtime: tokio::runtime::Handle,
}

impl Client {
    fn new(runtime: tokio::runtime::Handle) -> Self {
        Self {
            inner: Arc::new(
                opentelemetry_http::hyper::HyperClient::with_default_connector(
                    EXPORT_TIMEOUT,
                    None,
                ),
            ),
            runtime,
        }
    }
}

#[async_trait::async_trait]
impl HttpClient for Client {
    async fn send_bytes(&self, request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        let inner = self.inner.clone();

        self.runtime
            .spawn(async move { inner.send_bytes(request).await })
            .await?
    }
}

/// **The signal path is ours to add.** `with_endpoint` is taken verbatim as the
/// full URL — measured, a collector configured with the base address alone
/// receives every batch on `/` and answers 404, in the background, for the life
/// of the process. Only the value read from `OTEL_EXPORTER_OTLP_ENDPOINT` has
/// the path appended for it, and this one is read through `config` instead.
fn address(endpoint: &str, signal: &str) -> String {
    format!("{}/v1/{signal}", endpoint.trim_end_matches('/'))
}

/// What every signal is grouped by. `service.name` and nothing else, as the
/// service this replaces sets.
fn resource(service: &str) -> Resource {
    Resource::builder()
        .with_attribute(opentelemetry::KeyValue::new(
            SERVICE_NAME,
            service.to_owned(),
        ))
        .build()
}

fn sampler(settings: &Settings) -> Sampler {
    let ratio = settings.ratio;

    match settings.sampler {
        Sampling::AlwaysOn => Sampler::AlwaysOn,
        Sampling::AlwaysOff => Sampler::AlwaysOff,
        Sampling::TraceIdRatio => Sampler::TraceIdRatioBased(ratio),
        Sampling::ParentBasedAlwaysOn => Sampler::ParentBased(Box::new(Sampler::AlwaysOn)),
        Sampling::ParentBasedAlwaysOff => Sampler::ParentBased(Box::new(Sampler::AlwaysOff)),
        Sampling::ParentBasedTraceIdRatio => {
            Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(ratio)))
        }
    }
}

pub(crate) struct Providers {
    pub(crate) traces: SdkTracerProvider,
    pub(crate) meters: SdkMeterProvider,
    pub(crate) loggers: SdkLoggerProvider,
}

pub(crate) fn build(
    settings: &Settings,
    runtime: tokio::runtime::Handle,
) -> Result<Providers, opentelemetry_otlp::ExporterBuildError> {
    let resource = resource(&settings.service);

    let spans = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(address(&settings.endpoint, "traces"))
        .with_http_client(Client::new(runtime.clone()))
        .build()?;
    let traces = SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_sampler(sampler(settings))
        .with_batch_exporter(spans)
        .build();

    let measurements = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_endpoint(address(&settings.endpoint, "metrics"))
        .with_http_client(Client::new(runtime.clone()))
        .build()?;
    let meters = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_reader(
            PeriodicReader::builder(measurements)
                .with_interval(EXPORT_INTERVAL)
                .build(),
        )
        .build();

    let records = opentelemetry_otlp::LogExporter::builder()
        .with_http()
        .with_endpoint(address(&settings.endpoint, "logs"))
        .with_http_client(Client::new(runtime))
        .build()?;
    let loggers = SdkLoggerProvider::builder()
        .with_resource(resource)
        .with_batch_exporter(records)
        .build();

    Ok(Providers {
        traces,
        meters,
        loggers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_signal_the_collector_is_listening_for() {
        assert_eq!(
            address("http://telemetry:4318", "traces"),
            "http://telemetry:4318/v1/traces"
        );
    }

    #[test]
    fn a_trailing_slash_does_not_become_a_second_one() {
        assert_eq!(
            address("http://telemetry:4318/", "logs"),
            "http://telemetry:4318/v1/logs"
        );
    }

    #[test]
    fn a_ratio_only_reaches_the_samplers_that_take_one() {
        let settings = |sampler| Settings {
            service: "api".to_owned(),
            endpoint: "http://localhost:4318".to_owned(),
            sampler,
            ratio: 0.25,
            level: tracing::level_filters::LevelFilter::INFO,
            pretty: false,
            disabled: false,
        };

        assert!(matches!(
            sampler(&settings(Sampling::TraceIdRatio)),
            Sampler::TraceIdRatioBased(0.25)
        ));
        assert!(matches!(
            sampler(&settings(Sampling::AlwaysOn)),
            Sampler::AlwaysOn
        ));
        assert!(matches!(
            sampler(&settings(Sampling::ParentBasedAlwaysOff)),
            Sampler::ParentBased(_)
        ));
    }
}
