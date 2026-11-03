//! The loop wired to scripted ports, driven one iteration at a time.

use std::sync::Arc;

use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use tokio_util::sync::CancellationToken;

use crate::chain::FakeChainClient;
use crate::cursor::Cursor;
use crate::cursor::in_memory::InMemoryCursorStore;
use crate::engine::{Indexer, IndexerStatus, Instruments, Iteration, Options, Pipeline};
use crate::fixtures::{CHAIN_ID, Trace};
use crate::processors::Outcome;
use crate::processors::scripted::ScriptedBlockProcessor;
use crate::reorg::scripted::ScriptedReorgDetector;

pub(crate) const HEAD: u64 = 1_000;
pub(crate) const START: u64 = 100;
pub(crate) const MAX_RANGE: u64 = 50;

pub(crate) fn options() -> Options {
    Options {
        chain_id: CHAIN_ID,
        start_block: START,
        max_range_size: MAX_RANGE,
        poll_interval: std::time::Duration::from_millis(5),
    }
}

pub(crate) struct Fixture {
    pub(crate) indexer: Indexer,
    pub(crate) chain_client: Arc<FakeChainClient>,
    pub(crate) reorg_detector: ScriptedReorgDetector,
    pub(crate) cursor_store: InMemoryCursorStore,
    pub(crate) processors: Vec<ScriptedBlockProcessor>,
    pub(crate) status: Arc<IndexerStatus>,
    pub(crate) trace: Trace,
    pub(crate) cancel: CancellationToken,
    pub(crate) exporter: InMemoryMetricExporter,
    pub(crate) provider: SdkMeterProvider,
}

pub(crate) struct Setup {
    pub(crate) options: Options,
    pub(crate) head: u64,
    pub(crate) processors: usize,
    pub(crate) stored: Option<Cursor>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            options: options(),
            head: HEAD,
            processors: 1,
            stored: None,
        }
    }
}

pub(crate) fn fixture(setup: Setup) -> Fixture {
    let trace = Trace::default();
    let chain_client = Arc::new(FakeChainClient::at_head(setup.head));
    let reorg_detector = ScriptedReorgDetector::new(trace.clone());
    let mut cursor_store = InMemoryCursorStore::traced(trace.clone());
    if let Some(cursor) = setup.stored {
        cursor_store = cursor_store.holding(cursor);
    }
    let processors: Vec<_> = (1..=setup.processors)
        .map(|nth| ScriptedBlockProcessor::new(&format!("p{nth}"), trace.clone()))
        .collect();

    let status = Arc::new(IndexerStatus::new());
    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_reader(PeriodicReader::builder(exporter.clone()).build())
        .build();
    let instruments = Instruments::new(
        &provider.meter("test"),
        setup.options.chain_id,
        setup.options.max_range_size,
        Arc::clone(&status),
    );
    let cancel = CancellationToken::new();

    let indexer = Indexer::new(Pipeline {
        options: setup.options,
        chain_client: chain_client.clone(),
        reorg_detector: Box::new(reorg_detector.clone()),
        cursor_store: Box::new(cursor_store.clone()),
        processors: processors
            .iter()
            .map(|processor| Box::new(processor.clone()) as _)
            .collect(),
        status: Arc::clone(&status),
        instruments,
        cancel: cancel.clone(),
    });

    Fixture {
        indexer,
        chain_client,
        reorg_detector,
        cursor_store,
        processors,
        status,
        trace,
        cancel,
        exporter,
        provider,
    }
}

pub(crate) fn plain() -> Fixture {
    fixture(Setup::default())
}

pub(crate) fn retry(reason: &str) -> Outcome {
    Outcome::Retry {
        reason: reason.to_owned(),
        narrow_range: false,
    }
}

pub(crate) fn narrowing(reason: &str) -> Outcome {
    Outcome::Retry {
        reason: reason.to_owned(),
        narrow_range: true,
    }
}

pub(crate) fn failed(reason: &str) -> Outcome {
    Outcome::Failed {
        reason: reason.to_owned(),
    }
}

pub(crate) fn reason(result: &Iteration) -> String {
    match result {
        Iteration::Retry { reason } | Iteration::Failed { reason } => reason.clone(),
        other => panic!("expected a reason: {other:?}"),
    }
}
