//! The loop and [`HashChainReorgDetector`] together: whether the loop's ordering
//! and the detector's window agree, including across a restart.

use std::sync::Arc;

use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use tokio_util::sync::CancellationToken;

use crate::chain::FakeChainClient;
use crate::cursor::CursorStore as _;
use crate::cursor::in_memory::InMemoryCursorStore;
use crate::engine::harness::options;
use crate::engine::{Indexer, IndexerStatus, Instruments, Iteration, Options, Pipeline};
use crate::fixtures::{CHAIN_ID, Trace};
use crate::processors::scripted::ScriptedBlockProcessor;
use crate::reorg::in_memory::InMemoryBlockHeaderStore;
use crate::reorg::{BlockHeaderStore as _, HashChainReorgDetector, ReorgDetector as _, Verdict};

const DEPTH: u64 = 10;

/// What a restart keeps: the chain and both stores outlive any one loop.
#[derive(Clone)]
struct Durable {
    chain_client: Arc<FakeChainClient>,
    block_header_store: InMemoryBlockHeaderStore,
    cursor_store: InMemoryCursorStore,
}

impl Durable {
    fn at_head(head: u64) -> Self {
        Self {
            chain_client: Arc::new(FakeChainClient::at_head(head)),
            block_header_store: InMemoryBlockHeaderStore::new(),
            cursor_store: InMemoryCursorStore::new(),
        }
    }

    fn detector(&self) -> HashChainReorgDetector {
        HashChainReorgDetector::new(
            CHAIN_ID,
            DEPTH,
            self.chain_client.clone(),
            Box::new(self.block_header_store.clone()),
        )
    }

    fn indexer(&self, start_block: u64) -> (Indexer, ScriptedBlockProcessor) {
        let processor = ScriptedBlockProcessor::new("p1", Trace::default());
        let status = Arc::new(IndexerStatus::new());
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(InMemoryMetricExporter::default()).build())
            .build();
        let options = Options {
            start_block,
            ..options()
        };

        let indexer = Indexer::new(Pipeline {
            options,
            chain_client: self.chain_client.clone(),
            reorg_detector: Box::new(self.detector()),
            cursor_store: Box::new(self.cursor_store.clone()),
            processors: vec![Box::new(processor.clone())],
            status: Arc::clone(&status),
            instruments: Instruments::new(
                &provider.meter("test"),
                CHAIN_ID,
                options.max_range_size,
                status,
            ),
            cancel: CancellationToken::new(),
        });

        (indexer, processor)
    }

    async fn retained(&self) -> Vec<u64> {
        let mut numbers: Vec<u64> = self
            .block_header_store
            .load(CHAIN_ID)
            .await
            .unwrap()
            .iter()
            .map(|held| held.number)
            .collect();
        numbers.sort_unstable();

        numbers
    }

    fn last_saved(&self) -> Option<u64> {
        self.cursor_store
            .saved()
            .last()
            .map(|cursor| cursor.last_block)
    }
}

async fn catch_up(indexer: &mut Indexer, within: usize) {
    for _ in 0..within {
        if indexer.run_once().await == Iteration::Idle {
            return;
        }
    }

    panic!("it did not catch up within {within} iterations");
}

#[tokio::test]
async fn walking_the_tip_leaves_a_contiguous_window() {
    let head = 40;
    let durable = Durable::at_head(head);
    let (mut indexer, _) = durable.indexer(30);

    catch_up(&mut indexer, 12).await;

    let window = durable.retained().await;
    assert!(
        window.windows(2).all(|pair| pair[1] == pair[0] + 1),
        "{window:?}"
    );
    assert_eq!(window.last(), Some(&head));
    assert_eq!(durable.last_saved(), Some(head));
}

#[tokio::test]
async fn unwinds_a_fork_the_detector_found_for_itself() {
    let durable = Durable::at_head(40);
    let (mut indexer, processor) = durable.indexer(30);
    catch_up(&mut indexer, 12).await;

    durable.chain_client.fork_above(38, b'b');
    durable.chain_client.set_head(41);
    let result = indexer.run_once().await;

    assert_eq!(result, Iteration::Progressed { cursor_at: 38 });
    assert_eq!(
        processor.calls().last().map(|call| call.operation),
        Some("reorg")
    );
    assert_eq!(durable.retained().await.last(), Some(&38));
    assert_eq!(durable.last_saved(), Some(38));
}

#[tokio::test]
async fn a_second_loop_resumes_from_what_the_first_one_retained() {
    let durable = Durable::at_head(40);
    let (mut first, _) = durable.indexer(30);
    catch_up(&mut first, 12).await;
    drop(first);

    durable.chain_client.set_head(42);
    let (mut second, processor) = durable.indexer(30);
    let result = second.run_once().await;

    assert_eq!(result, Iteration::Progressed { cursor_at: 41 });
    assert_eq!(processor.ranges(), vec![(41, 41)]);
}

/// The gap the loop can die in: the cursor landed, the rewind did not.
#[tokio::test]
async fn a_restart_re_reports_a_fork_whose_rewind_never_ran() {
    let durable = Durable::at_head(40);
    let (mut first, _) = durable.indexer(30);
    catch_up(&mut first, 12).await;
    drop(first);

    durable.chain_client.fork_above(38, b'b');
    let stored = durable.cursor_store.load(CHAIN_ID).await.unwrap();
    let verdict = durable.detector().bootstrap(stored.as_ref()).await.unwrap();
    assert!(matches!(verdict, Verdict::Reorg { .. }), "{verdict:?}");

    let (mut second, processor) = durable.indexer(30);
    let result = second.run_once().await;

    assert_eq!(result, Iteration::Progressed { cursor_at: 38 });
    assert_eq!(
        processor.calls().first().map(|call| call.operation),
        Some("reorg")
    );
}
