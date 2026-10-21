//! The loop, driven one iteration at a time through doubles.
//!
//! `run_once` is the unit nearly every case here calls: it needs no timers and
//! leaves no background task to wind down, and it records the state transition
//! the iteration implies.

use std::sync::Arc;

use alloy_primitives::B256;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use tokio_util::sync::CancellationToken;

use crate::chain::ChainClient;
use crate::cursor::Cursor;
use crate::doubles::{
    FakeChain, RecordingCursorStore, RecordingProcessor, ScriptedDetector, recorded, trace,
};
use crate::engine::status::IndexerState;
use crate::engine::{Indexer, IndexerStatus, Instruments, Iteration, Options, Pipeline};
use crate::fixtures::{CHAIN_ID, hash_of};
use crate::processors::{BlockProcessor, Outcome};
use crate::reorg::Verdict;

const HEAD: u64 = 1_000;
const START: u64 = 100;
const MAX_RANGE: u64 = 50;

fn options() -> Options {
    Options {
        chain_id: CHAIN_ID,
        start_block: START,
        max_range_size: MAX_RANGE,
        poll_interval: std::time::Duration::from_millis(5),
    }
}

/// Everything a case may want to reach into afterwards.
struct Fixture {
    indexer: Indexer,
    chain: Arc<FakeChain>,
    detector: Arc<ScriptedDetector>,
    cursors: Arc<RecordingCursorStore>,
    processors: Vec<Arc<RecordingProcessor>>,
    status: Arc<IndexerStatus>,
    trace: crate::doubles::detector::Trace,
    cancel: CancellationToken,
    exporter: InMemoryMetricExporter,
    provider: SdkMeterProvider,
}

struct Setup {
    options: Options,
    head: u64,
    processors: usize,
    stored: Option<Cursor>,
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

fn fixture(setup: Setup) -> Fixture {
    let shared = trace();
    let chain = Arc::new(FakeChain::at_head(setup.head));
    let detector = Arc::new(ScriptedDetector::new(Arc::clone(&shared)));
    let cursors = Arc::new(match setup.stored {
        Some(cursor) => RecordingCursorStore::traced_holding(cursor, Arc::clone(&shared)),
        None => RecordingCursorStore::traced(Arc::clone(&shared)),
    });
    let processors: Vec<Arc<RecordingProcessor>> = (1..=setup.processors)
        .map(|nth| {
            Arc::new(RecordingProcessor::new(
                &format!("p{nth}"),
                Arc::clone(&shared),
            ))
        })
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
        chain: Arc::clone(&chain) as Arc<dyn ChainClient>,
        detector: Box::new(Arc::clone(&detector)),
        cursors: Box::new(Arc::clone(&cursors)),
        processors: processors
            .iter()
            .map(|processor| Box::new(Arc::clone(processor)) as Box<dyn BlockProcessor>)
            .collect(),
        status: Arc::clone(&status),
        instruments,
        cancel: cancel.clone(),
    });

    Fixture {
        indexer,
        chain,
        detector,
        cursors,
        processors,
        status,
        trace: shared,
        cancel,
        exporter,
        provider,
    }
}

fn plain() -> Fixture {
    fixture(Setup::default())
}

fn retry(reason: &str) -> Outcome {
    Outcome::Retry {
        reason: reason.to_owned(),
        narrow_range: false,
    }
}

fn narrowing(reason: &str) -> Outcome {
    Outcome::Retry {
        reason: reason.to_owned(),
        narrow_range: true,
    }
}

fn failed(reason: &str) -> Outcome {
    Outcome::Failed {
        reason: reason.to_owned(),
    }
}

fn reason(result: &Iteration) -> String {
    match result {
        Iteration::Retry { reason } | Iteration::Failed { reason } => reason.clone(),
        other => panic!("expected a reason: {other:?}"),
    }
}

mod processors {
    use super::*;

    #[tokio::test]
    async fn hands_the_same_range_to_every_processor_in_registration_order() {
        let mut fixture = fixture(Setup {
            processors: 3,
            ..Setup::default()
        });

        let _ = fixture.indexer.run_once().await;

        for processor in &fixture.processors {
            assert_eq!(processor.ranges(), vec![(START, START)]);
        }
        assert_eq!(
            recorded(&fixture.trace)
                .iter()
                .filter(|entry| entry.contains("block-range"))
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                format!("p1.block-range({START},{START})"),
                format!("p2.block-range({START},{START})"),
                format!("p3.block-range({START},{START})"),
            ]
        );
    }

    #[tokio::test]
    async fn stops_at_the_first_processor_that_does_not_accept_the_range() {
        let mut fixture = fixture(Setup {
            processors: 3,
            ..Setup::default()
        });
        fixture.processors[1].queue([retry("busy")]);

        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.processors[0].ranges().len(), 1);
        assert_eq!(fixture.processors[1].ranges().len(), 1);
        assert!(fixture.processors[2].ranges().is_empty(), "it ran anyway");
    }

    #[tokio::test]
    async fn names_the_processor_that_refused() {
        let mut fixture = plain();
        fixture.processors[0].queue([failed("decoder is wrong")]);

        let result = fixture.indexer.run_once().await;

        assert_eq!(reason(&result), "p1: decoder is wrong");
    }

    /// Nothing is committed and nothing is saved when a processor holds the
    /// range, which is what makes the cursor the commit point.
    #[tokio::test]
    async fn commits_nothing_when_a_processor_holds_the_range() {
        let mut fixture = plain();
        fixture.processors[0].queue([retry("busy")]);

        let _ = fixture.indexer.run_once().await;

        assert!(fixture.detector.committed().is_empty());
        assert!(fixture.cursors.saved().is_empty());
    }
}

mod dispatch_shape {
    use super::*;

    #[tokio::test]
    async fn dispatches_a_full_width_range_while_catching_up() {
        let mut fixture = plain();
        fixture.detector.settled_through(HEAD);

        let _ = fixture.indexer.run_once().await;

        assert_eq!(
            fixture.processors[0].ranges(),
            vec![(START, START + MAX_RANGE - 1)]
        );
    }

    #[tokio::test]
    async fn dispatches_one_block_at_a_time_near_the_tip() {
        let mut fixture = plain();

        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.processors[0].ranges(), vec![(START, START)]);
    }

    /// Half of a straddling range would be covered by a finality assumption and
    /// half not, and the single hash committed for its top would sit inside the
    /// reorg window with no ancestry beneath it.
    #[tokio::test]
    async fn a_range_never_crosses_the_settled_boundary() {
        let mut fixture = plain();
        fixture.detector.settled_through(START + 10);

        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.processors[0].ranges(), vec![(START, START + 10)]);
    }

    #[tokio::test]
    async fn the_reorg_path_is_untouched_when_the_chain_simply_extends() {
        let mut fixture = plain();

        let _ = fixture.indexer.run_once().await;

        assert!(
            fixture.processors[0]
                .calls()
                .iter()
                .all(|call| call.operation == "block-range")
        );
        assert!(fixture.detector.rewound_to().is_empty());
    }

    /// The replacement range comes on the next iteration.
    #[tokio::test]
    async fn blocks_and_a_reorg_never_go_out_in_one_iteration() {
        let mut fixture = plain();
        fixture.detector.queue([Verdict::Reorg {
            first_invalid_block: START - 2,
            last_invalid_block: START,
            last_valid_hash: hash_of(b'a', START - 3),
        }]);

        let _ = fixture.indexer.run_once().await;

        let operations: Vec<_> = fixture.processors[0]
            .calls()
            .iter()
            .map(|call| call.operation)
            .collect();

        assert_eq!(operations, vec!["reorg"]);
    }
}

mod resume {
    use super::*;

    #[tokio::test]
    async fn bootstraps_once_and_before_reading_the_head() {
        let mut fixture = plain();

        let _ = fixture.indexer.run_once().await;
        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.detector.bootstrapped_with().len(), 1);

        let calls = fixture.chain.calls();
        let bootstrap = recorded(&fixture.trace)
            .iter()
            .position(|entry| entry == "detector.bootstrap")
            .expect("it bootstrapped");
        assert_eq!(bootstrap, 0, "{:?}", recorded(&fixture.trace));
        assert!(matches!(
            calls.first(),
            Some(crate::doubles::ChainCall::ChainId)
        ));
    }

    #[tokio::test]
    async fn a_stored_cursor_beats_the_start_block() {
        let mut fixture = fixture(Setup {
            stored: Some(Cursor {
                chain_id: CHAIN_ID,
                last_block: 500,
                last_hash: hash_of(b'a', 500),
            }),
            ..Setup::default()
        });

        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.processors[0].ranges(), vec![(501, 501)]);
    }

    /// Including when it sits below the configured floor: the floor is only
    /// consulted when the store has nothing.
    #[tokio::test]
    async fn a_stored_cursor_below_the_start_block_still_wins() {
        let mut fixture = fixture(Setup {
            stored: Some(Cursor {
                chain_id: CHAIN_ID,
                last_block: 10,
                last_hash: hash_of(b'a', 10),
            }),
            ..Setup::default()
        });

        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.processors[0].ranges(), vec![(11, 11)]);
    }

    #[tokio::test]
    async fn the_start_block_is_used_when_nothing_is_stored() {
        let mut fixture = plain();

        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.processors[0].ranges(), vec![(START, START)]);
        assert_eq!(fixture.detector.bootstrapped_with(), vec![None]);
    }

    /// The chain forked while the process was down. Nothing else would notice:
    /// the window starts empty, so without this the loop would carry on from
    /// the cursor onto a branch that lost.
    #[tokio::test]
    async fn unwinds_a_fork_reported_at_bootstrap() {
        let mut fixture = plain();
        fixture.detector.on_bootstrap(Verdict::Reorg {
            first_invalid_block: 498,
            last_invalid_block: 500,
            last_valid_hash: hash_of(b'a', 497),
        });

        let result = fixture.indexer.run_once().await;

        assert_eq!(result, Iteration::Progressed { cursor_at: 497 });
        assert_eq!(
            fixture.processors[0].calls()[0].operation,
            "reorg",
            "the discard was not dispatched first"
        );
        assert_eq!(fixture.cursors.saved().len(), 1);
        assert_eq!(fixture.detector.rewound_to(), vec![Some(497)]);
    }

    #[tokio::test]
    async fn refuses_to_resume_when_the_detector_cannot_place_the_fork() {
        let mut fixture = plain();
        fixture.detector.on_bootstrap(Verdict::Unrecoverable {
            reason: "nothing retained".to_owned(),
        });

        let result = fixture.indexer.run_once().await;

        assert_eq!(reason(&result), "cannot resume: nothing retained");
        assert!(fixture.processors[0].calls().is_empty());
    }

    /// `bootstrapped` is set only once the whole step succeeded, so a transient
    /// failure re-runs it rather than leaving the detector half-primed.
    #[tokio::test]
    async fn retries_the_whole_bootstrap_when_the_reorg_dispatch_is_held() {
        let mut fixture = plain();
        fixture.detector.on_bootstrap(Verdict::Reorg {
            first_invalid_block: 498,
            last_invalid_block: 500,
            last_valid_hash: hash_of(b'a', 497),
        });
        fixture.processors[0].queue([retry("busy")]);

        let _ = fixture.indexer.run_once().await;
        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.detector.bootstrapped_with().len(), 2);
    }
}

mod the_outcome_gates_the_cursor {
    use super::*;

    #[tokio::test]
    async fn saves_the_cursor_with_the_hash_of_the_range_top() {
        let mut fixture = plain();
        fixture.detector.settled_through(HEAD);

        let _ = fixture.indexer.run_once().await;

        let top = START + MAX_RANGE - 1;
        assert_eq!(
            fixture.cursors.saved(),
            vec![Cursor {
                chain_id: CHAIN_ID,
                last_block: top,
                last_hash: hash_of(b'a', top),
            }]
        );
    }

    /// **Processors, then the header, then the cursor.** A crash anywhere
    /// earlier replays the range rather than skipping it.
    #[tokio::test]
    async fn writes_the_cursor_last_on_the_forward_path() {
        let mut fixture = plain();

        let _ = fixture.indexer.run_once().await;

        let entries = recorded(&fixture.trace);
        let at = |prefix: &str| {
            entries
                .iter()
                .position(|entry| entry.contains(prefix))
                .unwrap_or_else(|| panic!("{prefix} is missing from {entries:?}"))
        };

        // The whole sequence, not two of its three steps: a crash between any
        // adjacent pair has to replay the range rather than skip it.
        assert!(
            at("block-range") < at("detector.commit") && at("detector.commit") < at("cursor.save"),
            "{entries:?}"
        );
    }

    #[tokio::test]
    async fn a_held_range_is_re_dispatched_identically() {
        let mut fixture = plain();
        fixture.processors[0].queue([retry("busy")]);

        let _ = fixture.indexer.run_once().await;
        let _ = fixture.indexer.run_once().await;

        assert_eq!(
            fixture.processors[0].ranges(),
            vec![(START, START), (START, START)]
        );
    }

    #[tokio::test]
    async fn nothing_further_is_dispatched_after_a_terminal_failure() {
        let mut fixture = plain();
        fixture.processors[0].queue([failed("unfixable")]);

        let _ = fixture.indexer.run_once().await;
        let again = fixture.indexer.run_once().await;

        assert_eq!(reason(&again), "p1: unfixable");
        assert_eq!(fixture.processors[0].ranges().len(), 1);
    }

    /// Indistinguishable from a transient fault at this level, and calling it
    /// terminal would let a passing bug stop indexing for good.
    #[tokio::test]
    async fn a_processor_that_refuses_is_retryable_rather_than_terminal() {
        let mut fixture = plain();
        fixture.processors[0].refuse_with("connection reset");

        let result = fixture.indexer.run_once().await;

        assert!(matches!(result, Iteration::Retry { .. }), "{result:?}");
        assert!(reason(&result).contains("p1 refused: connection reset"));
    }

    #[tokio::test]
    async fn narrowing_halves_the_dispatched_width() {
        let mut fixture = plain();
        fixture.detector.settled_through(HEAD);
        fixture.processors[0].queue([narrowing("range too wide")]);

        let _ = fixture.indexer.run_once().await;
        let _ = fixture.indexer.run_once().await;

        assert_eq!(
            fixture.processors[0].ranges(),
            vec![
                (START, START + MAX_RANGE - 1),
                (START, START + MAX_RANGE / 2 - 1)
            ]
        );
    }

    /// **How the commit point is proven.** A refused save leaves the loop
    /// pointing at the last position actually persisted, so the identical range
    /// goes out again.
    #[tokio::test]
    async fn a_refused_cursor_save_re_dispatches_the_same_range() {
        let mut fixture = plain();
        fixture.cursors.fail_save(1);

        let first = fixture.indexer.run_once().await;
        let _ = fixture.indexer.run_once().await;

        assert!(matches!(first, Iteration::Retry { .. }), "{first:?}");
        assert_eq!(
            fixture.processors[0].ranges(),
            vec![(START, START), (START, START)]
        );
    }
}

mod the_detector_owns_the_chains_shape {
    use super::*;

    #[tokio::test]
    async fn asks_about_every_header_it_commits() {
        let mut fixture = plain();

        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.detector.inspected().len(), 1);
        assert_eq!(fixture.detector.committed().len(), 1);
    }

    /// **Before dispatch, not alongside it.** Folding a range the detector was
    /// about to call stale is the thing this ordering prevents.
    #[tokio::test]
    async fn asks_before_dispatching_rather_than_alongside() {
        let mut fixture = plain();

        let _ = fixture.indexer.run_once().await;

        let entries = recorded(&fixture.trace);
        let inspected = entries
            .iter()
            .position(|entry| entry.starts_with("detector.inspect"))
            .expect("inspected");
        let dispatched = entries
            .iter()
            .position(|entry| entry.contains("block-range"))
            .expect("dispatched");

        assert!(inspected < dispatched, "{entries:?}");
    }

    /// One header for a thousand blocks, so a backfill asks about ranges rather
    /// than blocks.
    #[tokio::test]
    async fn asks_about_a_settled_range_once_and_not_per_block() {
        let mut fixture = plain();
        fixture.detector.settled_through(HEAD);

        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.detector.inspected().len(), 1);
        assert_eq!(fixture.chain.header_reads(), 1);
    }

    /// **Processors, then the cursor, then the detector.** The rewind discards
    /// the evidence of the abandoned branch, so it waits until the cursor is
    /// durable.
    #[tokio::test]
    async fn unwinds_in_the_order_processors_then_cursor_then_detector() {
        let mut fixture = plain();
        fixture.detector.queue([Verdict::Reorg {
            first_invalid_block: 98,
            last_invalid_block: 100,
            last_valid_hash: hash_of(b'a', 97),
        }]);

        let _ = fixture.indexer.run_once().await;

        let entries = recorded(&fixture.trace);
        let at = |prefix: &str| {
            entries
                .iter()
                .position(|entry| entry.contains(prefix))
                .unwrap_or_else(|| panic!("{prefix} is missing from {entries:?}"))
        };

        assert!(
            at("reorg(") < at("cursor.save") && at("cursor.save") < at("detector.rewind_to"),
            "{entries:?}"
        );
        assert_eq!(fixture.detector.rewound_to(), vec![Some(97)]);
    }

    /// Nothing is mutated until every processor has accepted the discard.
    #[tokio::test]
    async fn leaves_the_rewind_unapplied_when_a_processor_holds_the_discard() {
        let mut fixture = plain();
        fixture.detector.queue([Verdict::Reorg {
            first_invalid_block: 98,
            last_invalid_block: 100,
            last_valid_hash: hash_of(b'a', 97),
        }]);
        fixture.processors[0].queue([retry("busy")]);

        let _ = fixture.indexer.run_once().await;

        assert!(fixture.detector.rewound_to().is_empty());
        assert!(fixture.cursors.saved().is_empty());
    }

    /// The window still holds the run above an unchanged cursor, which is
    /// strictly more to work with than the reverse.
    #[tokio::test]
    async fn keeps_the_retained_headers_when_the_rewound_cursor_is_refused() {
        let mut fixture = plain();
        fixture.detector.queue([Verdict::Reorg {
            first_invalid_block: 98,
            last_invalid_block: 100,
            last_valid_hash: hash_of(b'a', 97),
        }]);
        fixture.cursors.fail_save(1);

        let _ = fixture.indexer.run_once().await;

        assert!(fixture.detector.rewound_to().is_empty());
    }

    #[tokio::test]
    async fn stops_when_the_fork_is_beyond_the_detectors_reach() {
        let mut fixture = plain();
        fixture.detector.queue([Verdict::Unrecoverable {
            reason: "below the safe head".to_owned(),
        }]);

        let result = fixture.indexer.run_once().await;

        assert_eq!(
            reason(&result),
            "reorg beyond the detector's reach: below the safe head"
        );
    }

    /// Genesis replaced underneath. There is no block below it to rewind the
    /// cursor to, and it answers rather than underflowing.
    #[tokio::test]
    async fn unwinds_to_nothing_when_genesis_itself_was_replaced() {
        let mut fixture = fixture(Setup {
            options: Options {
                start_block: 0,
                ..options()
            },
            ..Setup::default()
        });
        fixture.detector.queue([Verdict::Reorg {
            first_invalid_block: 0,
            last_invalid_block: 0,
            last_valid_hash: B256::ZERO,
        }]);

        let result = fixture.indexer.run_once().await;

        assert_eq!(result, Iteration::Progressed { cursor_at: 0 });
        assert_eq!(fixture.detector.rewound_to(), vec![None]);
        assert!(fixture.cursors.saved().is_empty(), "it saved below genesis");
    }
}

mod loop_guards {
    use super::*;

    #[tokio::test]
    async fn idles_when_the_start_block_is_ahead_of_the_chain() {
        let mut fixture = fixture(Setup {
            head: START - 1,
            ..Setup::default()
        });

        let result = fixture.indexer.run_once().await;

        assert_eq!(result, Iteration::Idle);
        assert!(fixture.processors[0].calls().is_empty());
    }

    #[tokio::test]
    async fn idles_once_it_has_caught_up() {
        let mut fixture = fixture(Setup {
            head: START,
            ..Setup::default()
        });

        let _ = fixture.indexer.run_once().await;
        let caught_up = fixture.indexer.run_once().await;

        assert_eq!(caught_up, Iteration::Idle);
    }

    /// Idling counts as progress: being current is the healthy steady state,
    /// and a quiet chain must not trip the stall signal.
    #[tokio::test]
    async fn an_idle_iteration_is_progress_for_the_stall_clock() {
        let mut fixture = fixture(Setup {
            head: START - 1,
            ..Setup::default()
        });

        let _ = fixture.indexer.run_once().await;

        let snapshot = fixture.status.snapshot();
        assert_eq!(snapshot.state, IndexerState::Running);
        assert_eq!(snapshot.consecutive_failures, 0);
    }

    /// A failover to a lagging node is otherwise indistinguishable from a
    /// reorg, and it can invert a range into `from > to`.
    #[tokio::test]
    async fn a_backwards_head_is_clamped_to_the_high_mark() {
        let mut fixture = fixture(Setup {
            head: START + 10,
            ..Setup::default()
        });
        fixture.detector.settled_through(HEAD);

        let _ = fixture.indexer.run_once().await;
        fixture.chain.set_head(START);
        let _ = fixture.indexer.run_once().await;

        assert_eq!(fixture.status.snapshot().head, Some(START + 10));
    }

    #[tokio::test]
    async fn stops_on_a_chain_id_mismatch() {
        let mut fixture = plain();
        fixture.chain.set_chain_id(CHAIN_ID + 1);

        let result = fixture.indexer.run_once().await;

        assert_eq!(
            reason(&result),
            format!(
                "chain id mismatch: configured {CHAIN_ID}, provider reports {}",
                CHAIN_ID + 1
            )
        );
    }

    #[tokio::test]
    async fn verifies_the_chain_id_once() {
        let mut fixture = plain();

        let _ = fixture.indexer.run_once().await;
        let _ = fixture.indexer.run_once().await;

        let asked = fixture
            .chain
            .calls()
            .iter()
            .filter(|call| matches!(call, crate::doubles::ChainCall::ChainId))
            .count();

        assert_eq!(asked, 1);
    }

    /// A node that cannot be reached is a retry, not a mismatch. Treating it as
    /// terminal would make a provider outage a crash loop.
    #[tokio::test]
    async fn an_unreachable_chain_retries_rather_than_stopping() {
        let mut fixture = plain();
        fixture.chain.fail_next(1);

        let result = fixture.indexer.run_once().await;

        assert!(matches!(result, Iteration::Retry { .. }), "{result:?}");
        assert!(reason(&result).starts_with("unhandled:"));
    }

    #[tokio::test]
    async fn a_processor_is_told_that_shutdown_began() {
        let mut fixture = plain();
        fixture.cancel.cancel();

        let _ = fixture.indexer.run_once().await;

        assert!(
            fixture.processors[0]
                .calls()
                .iter()
                .all(|call| call.cancelled)
        );
    }

    #[tokio::test]
    async fn the_loop_stops_when_cancelled() {
        let mut fixture = plain();
        fixture.cancel.cancel();

        fixture.indexer.run().await;

        assert_eq!(fixture.status.snapshot().state, IndexerState::Stopped);
    }

    /// Terminal, and the state says so rather than reading `stopped` — which is
    /// what the readiness check keys on.
    #[tokio::test]
    async fn a_terminal_failure_leaves_the_loop_failed_rather_than_stopped() {
        let mut fixture = plain();
        fixture.processors[0].queue([failed("unfixable")]);

        fixture.indexer.run().await;

        assert_eq!(fixture.status.snapshot().state, IndexerState::Failed);
    }

    /// Without this a quiet loop would sit out its whole poll interval before
    /// noticing it had been asked to stop.
    #[tokio::test(start_paused = true)]
    async fn shutdown_does_not_wait_out_the_poll_interval() {
        let mut fixture = fixture(Setup {
            head: START - 1,
            options: Options {
                poll_interval: std::time::Duration::from_secs(600),
                ..options()
            },
            ..Setup::default()
        });

        let started = tokio::time::Instant::now();
        let cancel = fixture.cancel.clone();
        let stopper = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            cancel.cancel();
        });

        fixture.indexer.run().await;
        stopper.await.unwrap();

        assert!(
            started.elapsed() < std::time::Duration::from_secs(600),
            "{:?}",
            started.elapsed()
        );
    }

    /// 500ms doubling to a 30s ceiling, and the ends of it: no wait at all
    /// after progress, the poll interval after idling.
    #[test]
    fn the_backoff_doubles_to_a_ceiling() {
        use crate::engine::indexer::backoff;

        let waits: Vec<_> = (1..=8).map(backoff).collect();

        assert_eq!(
            waits,
            vec![
                std::time::Duration::from_millis(500),
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(2),
                std::time::Duration::from_secs(4),
                std::time::Duration::from_secs(8),
                std::time::Duration::from_secs(16),
                std::time::Duration::from_secs(30),
                std::time::Duration::from_secs(30),
            ]
        );
        // The count the loop has before its first failure is recorded.
        assert_eq!(backoff(0), std::time::Duration::from_millis(500));
    }

    /// And the loop actually waits it, which the formula on its own does not
    /// say.
    #[tokio::test(start_paused = true)]
    async fn the_loop_waits_its_backoff_between_attempts() {
        let mut fixture = plain();
        fixture.processors[0].refuse_with("provider down");

        let cancel = fixture.cancel.clone();
        let stopper = tokio::spawn(async move {
            // Past the first backoff and into the second, so at least one was
            // waited out in full.
            tokio::time::sleep(std::time::Duration::from_millis(900)).await;
            cancel.cancel();
        });

        let started = tokio::time::Instant::now();
        fixture.indexer.run().await;
        stopper.await.unwrap();

        assert!(
            started.elapsed() >= std::time::Duration::from_millis(500),
            "it did not wait: {:?}",
            started.elapsed()
        );
        assert!(
            fixture.processors[0].calls().len() >= 2,
            "it stopped after one attempt"
        );
    }
}

mod what_the_loop_reports {
    use super::*;
    use std::collections::BTreeSet;

    use opentelemetry_sdk::metrics::data::AggregatedMetrics;

    /// **Resets before flushing.** `get_finished_metrics` accumulates every
    /// flush, so without this a second read sees the first collection's points
    /// as well and `[0]` is whichever is stale.
    fn collected(fixture: &Fixture) -> Vec<opentelemetry_sdk::metrics::data::ResourceMetrics> {
        fixture.exporter.reset();
        fixture.provider.force_flush().expect("a collection");

        fixture
            .exporter
            .get_finished_metrics()
            .expect("the exported batches")
    }

    fn instrument_names(fixture: &Fixture) -> BTreeSet<String> {
        collected(fixture)
            .iter()
            .flat_map(|batch| batch.scope_metrics().collect::<Vec<_>>())
            .flat_map(|scope| scope.metrics().collect::<Vec<_>>())
            .map(|metric| metric.name().to_owned())
            .collect()
    }

    /// The whole set, not a lookup, so an instrument arriving without a panel
    /// or leaving with one pointed at it fails here.
    /// Every instrument is *recorded into* first, because a histogram with no
    /// data point is absent from a collection rather than empty in it — so the
    /// set would be short by one and say nothing about why.
    #[tokio::test]
    async fn names_every_instrument_the_engine_records() {
        let mut fixture = plain();
        fixture.detector.queue([Verdict::Reorg {
            first_invalid_block: 98,
            last_invalid_block: 100,
            last_valid_hash: hash_of(b'a', 97),
        }]);

        let _ = fixture.indexer.run_once().await;
        let _ = fixture.indexer.run_once().await;

        assert_eq!(
            instrument_names(&fixture),
            BTreeSet::from(
                [
                    "indexer.iterations",
                    "indexer.iteration.duration",
                    "indexer.processor.duration",
                    "indexer.reorg.depth",
                    "indexer.blocks.indexed",
                    "indexer.cursor.block",
                    "indexer.head.block",
                    "indexer.lag.blocks",
                    "indexer.progress.age",
                    "indexer.consecutive_failures",
                    "indexer.state",
                    "indexer.range.size",
                ]
                .map(str::to_owned)
            )
        );
    }

    fn points(fixture: &Fixture, name: &str) -> Vec<(Vec<String>, u64)> {
        collected(fixture)
            .iter()
            .flat_map(|batch| batch.scope_metrics().collect::<Vec<_>>())
            .flat_map(|scope| scope.metrics().collect::<Vec<_>>())
            .filter(|metric| metric.name() == name)
            .filter_map(|metric| match metric.data() {
                AggregatedMetrics::U64(data) => Some(data),
                _ => None,
            })
            .flat_map(|data| match data {
                opentelemetry_sdk::metrics::data::MetricData::Gauge(gauge) => gauge
                    .data_points()
                    .map(|point| {
                        (
                            point
                                .attributes()
                                .map(|attribute| {
                                    format!("{}={}", attribute.key, attribute.value.as_str())
                                })
                                .collect(),
                            point.value(),
                        )
                    })
                    .collect::<Vec<_>>(),
                opentelemetry_sdk::metrics::data::MetricData::Sum(sum) => sum
                    .data_points()
                    .map(|point| {
                        (
                            point
                                .attributes()
                                .map(|attribute| {
                                    format!("{}={}", attribute.key, attribute.value.as_str())
                                })
                                .collect(),
                            point.value(),
                        )
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect()
    }

    /// Reporting 0 would draw a cliff on the graph at every restart and make
    /// the lag briefly enormous. Reporting nothing draws a gap, which is what
    /// it is.
    #[tokio::test]
    async fn an_unindexed_chain_publishes_no_cursor_point_rather_than_a_zero() {
        let fixture = plain();

        assert!(points(&fixture, "indexer.cursor.block").is_empty());
        assert!(points(&fixture, "indexer.lag.blocks").is_empty());
        // The ones that always have an answer are still there.
        assert_eq!(points(&fixture, "indexer.consecutive_failures").len(), 1);
    }

    /// One snapshot per collection, so a panel subtracting the published cursor
    /// from the published head agrees with the published lag.
    #[tokio::test]
    async fn one_collection_cannot_disagree_with_itself() {
        let mut fixture = plain();
        fixture.detector.settled_through(HEAD);
        let _ = fixture.indexer.run_once().await;

        let cursor = points(&fixture, "indexer.cursor.block")[0].1;
        let head = points(&fixture, "indexer.head.block")[0].1;
        let lag = points(&fixture, "indexer.lag.blocks")[0].1;

        assert_eq!(lag, head - cursor);
    }

    /// One series per state, so `failed` is alertable without parsing a string.
    #[tokio::test]
    async fn publishes_one_series_per_state() {
        let mut fixture = plain();
        fixture.processors[0].queue([failed("unfixable")]);
        let _ = fixture.indexer.run_once().await;

        let series = points(&fixture, "indexer.state");
        let hot: Vec<_> = series
            .iter()
            .filter(|(_, value)| *value == 1)
            .flat_map(|(attributes, _)| attributes.clone())
            .filter(|attribute| attribute.starts_with("state="))
            .collect();

        assert_eq!(series.len(), 5, "{series:?}");
        assert_eq!(hot, vec!["state=failed".to_owned()]);
    }

    #[tokio::test]
    async fn the_range_gauge_follows_a_narrowing() {
        let mut fixture = plain();
        fixture.detector.settled_through(HEAD);
        fixture.processors[0].queue([narrowing("too wide")]);

        assert_eq!(points(&fixture, "indexer.range.size")[0].1, MAX_RANGE);
        let _ = fixture.indexer.run_once().await;

        assert_eq!(points(&fixture, "indexer.range.size")[0].1, MAX_RANGE / 2);
    }

    #[tokio::test]
    async fn counts_the_blocks_it_indexed() {
        let mut fixture = plain();
        fixture.detector.settled_through(HEAD);

        let _ = fixture.indexer.run_once().await;

        assert_eq!(points(&fixture, "indexer.blocks.indexed")[0].1, MAX_RANGE);
    }

    #[tokio::test]
    async fn counts_an_iteration_under_its_outcome() {
        let mut fixture = plain();
        fixture.processors[0].queue([retry("busy")]);

        let _ = fixture.indexer.run_once().await;

        let counted = points(&fixture, "indexer.iterations");
        assert!(
            counted.iter().any(|(attributes, value)| attributes
                .contains(&"outcome=retry".to_owned())
                && *value == 1),
            "{counted:?}"
        );
    }

    /// Its count is how often, so there is no separate reorg counter.
    #[tokio::test]
    async fn records_how_deep_a_reorg_reached() {
        let mut fixture = plain();
        fixture.detector.queue([Verdict::Reorg {
            first_invalid_block: 98,
            last_invalid_block: 100,
            last_valid_hash: hash_of(b'a', 97),
        }]);

        let _ = fixture.indexer.run_once().await;
        fixture.provider.force_flush().expect("a collection");

        let found = fixture
            .exporter
            .get_finished_metrics()
            .expect("batches")
            .iter()
            .flat_map(|batch| batch.scope_metrics().collect::<Vec<_>>())
            .flat_map(|scope| scope.metrics().collect::<Vec<_>>())
            .any(|metric| metric.name() == "indexer.reorg.depth");

        assert!(found);
    }
}

mod spans {
    //! What a trace backend sees, read back from an exporter.

    use opentelemetry::trace::{Status, TracerProvider as _};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
    use tracing::instrument::WithSubscriber as _;
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;

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
        fixture.detector.queue([Verdict::Reorg {
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
}

mod over_the_real_detector {
    //! The loop and [`HashChainReorgDetector`] together, which is the pair a
    //! scripted double cannot answer for: whether the loop's ordering and the
    //! detector's window agree, including across a restart.

    use super::*;
    use crate::cursor::CursorStore as _;
    use crate::doubles::InMemoryBlockHeaderStore;
    use crate::reorg::{BlockHeaderStore as _, HashChainReorgDetector, ReorgDetector};

    const DEPTH: u64 = 10;

    struct Real {
        indexer: Indexer,
        chain: Arc<FakeChain>,
        window: Arc<InMemoryBlockHeaderStore>,
        cursors: Arc<RecordingCursorStore>,
        processor: Arc<RecordingProcessor>,
    }

    /// Both stores are shared so a second loop can be stood up over the state
    /// the first one left — which is what a restart is.
    fn real(
        chain: &Arc<FakeChain>,
        window: &Arc<InMemoryBlockHeaderStore>,
        cursors: &Arc<RecordingCursorStore>,
        start_block: u64,
    ) -> Real {
        let shared = trace();
        let processor = Arc::new(RecordingProcessor::new("p1", shared));
        let status = Arc::new(IndexerStatus::new());
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(InMemoryMetricExporter::default()).build())
            .build();

        let detector = HashChainReorgDetector::new(
            CHAIN_ID,
            DEPTH,
            Arc::clone(chain) as Arc<dyn ChainClient>,
            Box::new(Arc::clone(window)),
        );

        let options = Options {
            start_block,
            ..options()
        };
        let indexer = Indexer::new(Pipeline {
            options,
            chain: Arc::clone(chain) as Arc<dyn ChainClient>,
            detector: Box::new(detector),
            cursors: Box::new(Arc::clone(cursors)),
            processors: vec![Box::new(Arc::clone(&processor)) as Box<dyn BlockProcessor>],
            status: Arc::clone(&status),
            instruments: Instruments::new(
                &provider.meter("test"),
                CHAIN_ID,
                options.max_range_size,
                status,
            ),
            cancel: CancellationToken::new(),
        });

        Real {
            indexer,
            chain: Arc::clone(chain),
            window: Arc::clone(window),
            cursors: Arc::clone(cursors),
            processor,
        }
    }

    fn fresh(head: u64, start_block: u64) -> Real {
        real(
            &Arc::new(FakeChain::at_head(head)),
            &Arc::new(InMemoryBlockHeaderStore::new()),
            &Arc::new(RecordingCursorStore::new()),
            start_block,
        )
    }

    /// Runs until it idles rather than a fixed number of times: the first
    /// iteration bootstraps *and* indexes, so a count is one off before it is
    /// anything else.
    async fn catch_up(fixture: &mut Real) {
        for _ in 0..64 {
            if fixture.indexer.run_once().await == Iteration::Idle {
                return;
            }
        }

        panic!("it never caught up");
    }

    async fn retained(window: &Arc<InMemoryBlockHeaderStore>) -> Vec<u64> {
        let mut numbers: Vec<u64> = window
            .load(CHAIN_ID)
            .await
            .unwrap()
            .iter()
            .map(|held| held.number)
            .collect();
        numbers.sort_unstable();

        numbers
    }

    /// The window the loop leaves behind is the one the detector's own cases
    /// assert on, which is only true if the loop commits what it says it does.
    #[tokio::test]
    async fn walking_the_tip_leaves_a_contiguous_window() {
        let head = 40;
        let mut fixture = fresh(head, 30);

        catch_up(&mut fixture).await;

        let window = retained(&fixture.window).await;

        assert!(
            window.windows(2).all(|pair| pair[1] == pair[0] + 1),
            "{window:?}"
        );
        assert_eq!(window.last(), Some(&head));
        assert_eq!(
            fixture
                .cursors
                .saved()
                .last()
                .map(|cursor| cursor.last_block),
            Some(head)
        );
    }

    /// End to end: the chain forks, the detector finds it from headers it read,
    /// the loop dispatches the discard, rewinds the cursor and only then lets
    /// the window be truncated.
    #[tokio::test]
    async fn unwinds_a_fork_the_detector_found_for_itself() {
        let mut fixture = fresh(40, 30);
        catch_up(&mut fixture).await;

        fixture.chain.fork_above(38, b'b');
        fixture.chain.set_head(41);
        let result = fixture.indexer.run_once().await;

        assert_eq!(result, Iteration::Progressed { cursor_at: 38 });
        assert_eq!(
            fixture.processor.calls().last().map(|call| call.operation),
            Some("reorg")
        );
        assert_eq!(retained(&fixture.window).await.last(), Some(&38));
        assert_eq!(
            fixture
                .cursors
                .saved()
                .last()
                .map(|cursor| cursor.last_block),
            Some(38)
        );
    }

    /// A second loop over the state the first left. The resume point is vetted
    /// from the retained window rather than taken on trust.
    #[tokio::test]
    async fn a_second_loop_resumes_from_what_the_first_one_retained() {
        let chain = Arc::new(FakeChain::at_head(40));
        let window = Arc::new(InMemoryBlockHeaderStore::new());
        let cursors = Arc::new(RecordingCursorStore::new());

        let mut first = real(&chain, &window, &cursors, 30);
        catch_up(&mut first).await;
        drop(first);

        chain.set_head(42);
        let mut second = real(&chain, &window, &cursors, 30);
        let result = second.indexer.run_once().await;

        assert_eq!(result, Iteration::Progressed { cursor_at: 41 });
        assert_eq!(second.processor.ranges(), vec![(41, 41)]);
    }

    /// The gap the loop can die in: the cursor landed, the rewind did not. A
    /// restart re-reports the same fork and the discard is idempotent.
    #[tokio::test]
    async fn a_restart_re_reports_a_fork_whose_rewind_never_ran() {
        let chain = Arc::new(FakeChain::at_head(40));
        let window = Arc::new(InMemoryBlockHeaderStore::new());
        let cursors = Arc::new(RecordingCursorStore::new());

        let mut first = real(&chain, &window, &cursors, 30);
        catch_up(&mut first).await;

        // The fork is found and the cursor rewound, but the window keeps the
        // abandoned run: that is the state a crash between the two leaves.
        chain.fork_above(38, b'b');
        let detector = HashChainReorgDetector::new(
            CHAIN_ID,
            DEPTH,
            Arc::clone(&chain) as Arc<dyn ChainClient>,
            Box::new(Arc::clone(&window)),
        );
        let verdict = detector
            .bootstrap(cursors.load(CHAIN_ID).await.unwrap().as_ref())
            .await
            .unwrap();
        assert!(
            matches!(verdict, crate::reorg::Verdict::Reorg { .. }),
            "{verdict:?}"
        );
        drop(first);

        let mut second = real(&chain, &window, &cursors, 30);
        let result = second.indexer.run_once().await;

        assert_eq!(result, Iteration::Progressed { cursor_at: 38 });
        assert_eq!(
            second.processor.calls().first().map(|call| call.operation),
            Some("reorg")
        );
    }
}
