//! What a trace backend sees, read back from an exporter.

use opentelemetry::trace::{Status, TracerProvider as _};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::layer::SubscriberExt as _;

use crate::engine::Indexer;
use crate::engine::harness::{START, failed, plain, retry};
use crate::fixtures::hash_of;
use crate::reorg::Verdict;

/// Makes every callsite in this crate interesting, once.
///
/// `tracing` caches callsite interest from the *global* subscriber, which is
/// `NoSubscriber` here and answers "never" — so without this, whether a
/// scoped subscriber sees a span depends on which case ran first.
fn interesting() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();

    ONCE.get_or_init(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(tracing::level_filters::LevelFilter::TRACE),
        );
    });
}

/// One iteration, with its spans. `with_subscriber` on the future rather
/// than `with_default` around it, because this is already inside a runtime
/// and building a second one inside a sync scope would panic.
async fn traced(indexer: &mut Indexer) -> Vec<SpanData> {
    interesting();

    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("indexing")));

    let _ = indexer.run_once().with_subscriber(subscriber).await;
    provider.force_flush().unwrap();

    exporter.get_finished_spans().unwrap()
}

fn named<'a>(spans: &'a [SpanData], name: &str) -> Option<&'a SpanData> {
    spans.iter().find(|span| span.name == name)
}

fn attribute(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == key)
        .map(|attribute| attribute.value.as_str().into_owned())
}

#[tokio::test]
async fn an_iteration_is_one_span_carrying_its_outcome() {
    let mut fixture = plain();

    let spans = traced(&mut fixture.indexer).await;
    let iteration = named(&spans, "indexer.iterate").expect("an iteration span");

    assert_eq!(
        attribute(iteration, "indexer.outcome").as_deref(),
        Some("progressed")
    );
    assert_eq!(
        attribute(iteration, "indexer.cursor.block").as_deref(),
        Some(START.to_string().as_str())
    );
    assert_eq!(iteration.status, Status::Unset);
}

/// Dispatch is sequential, so the slowest processor is the whole loop's
/// latency — which is only readable if its span sits under the iteration's.
#[tokio::test]
async fn a_processor_span_nests_under_the_iteration() {
    let mut fixture = plain();

    let spans = traced(&mut fixture.indexer).await;
    let iteration = named(&spans, "indexer.iterate").expect("an iteration span");
    let processor = named(&spans, "processor.p1").expect("a processor span");

    assert_eq!(processor.parent_span_id, iteration.span_context.span_id());
    assert_eq!(
        attribute(processor, "operation").as_deref(),
        Some("block-range")
    );
    assert_eq!(attribute(processor, "outcome").as_deref(), Some("ok"));
}

/// A retry is routine to the loop and an error to a trace backend, and the
/// second framing is the useful one: it is what makes the failing
/// iterations filterable out of thousands of healthy ones.
#[tokio::test]
async fn a_retrying_iteration_is_an_error_to_a_trace_backend() {
    let mut fixture = plain();
    fixture.processors[0].queue([retry("busy")]);

    let spans = traced(&mut fixture.indexer).await;
    let iteration = named(&spans, "indexer.iterate").expect("an iteration span");

    assert!(
        matches!(iteration.status, Status::Error { .. }),
        "{:?}",
        iteration.status
    );
    assert_eq!(
        attribute(iteration, "indexer.outcome").as_deref(),
        Some("retry")
    );
}

/// A stopped loop answering "still stopped" every few seconds is not an
/// iteration, and a span for each would fill a trace backend with nothing.
#[tokio::test]
async fn a_stopped_loop_opens_no_span_at_all() {
    let mut fixture = plain();
    fixture.processors[0].queue([failed("unfixable")]);
    let _ = fixture.indexer.run_once().await;

    let spans = traced(&mut fixture.indexer).await;

    assert!(spans.is_empty(), "{spans:?}");
}

/// As an **event** rather than a span of its own, because a fork is
/// something that happened inside an iteration rather than a phase beside
/// it — and a span whose parent is the thing that matters adds a level to
/// every trace to say so.
#[tokio::test]
async fn a_reorg_is_an_event_on_the_iteration_rather_than_a_span() {
    let mut fixture = plain();
    fixture.reorg_detector.queue([Verdict::Reorg {
        first_invalid_block: 98,
        last_invalid_block: 100,
        last_valid_hash: hash_of(b'a', 97),
    }]);

    let spans = traced(&mut fixture.indexer).await;
    let iteration = named(&spans, "indexer.iterate").expect("an iteration span");

    assert!(
        named(&spans, "reorg").is_none(),
        "a reorg opened a span of its own"
    );

    let event = iteration
        .events
        .iter()
        .find(|event| event.name == "reorg")
        .expect("a reorg event");
    let depth = event
        .attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == "depth")
        .map(|attribute| attribute.value.as_str().into_owned());

    assert_eq!(depth.as_deref(), Some("3"));
}
