use alloy_primitives::B256;

use crate::engine::harness::{HEAD, Setup, fixture, options, plain, reason, retry};
use crate::engine::{Iteration, Options};
use crate::fixtures::hash_of;
use crate::reorg::Verdict;

#[tokio::test]
async fn asks_about_every_header_it_commits() {
    let mut fixture = plain();

    let _ = fixture.indexer.run_once().await;

    assert_eq!(fixture.reorg_detector.inspected().len(), 1);
    assert_eq!(fixture.reorg_detector.committed().len(), 1);
}

/// **Before dispatch, not alongside it.** Folding a range the detector was
/// about to call stale is the thing this ordering prevents.
#[tokio::test]
async fn asks_before_dispatching_rather_than_alongside() {
    let mut fixture = plain();

    let _ = fixture.indexer.run_once().await;

    let entries = fixture.trace.recorded();
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
    fixture.reorg_detector.settled_through(HEAD);

    let _ = fixture.indexer.run_once().await;

    assert_eq!(fixture.reorg_detector.inspected().len(), 1);
    assert_eq!(fixture.chain_client.header_reads(), 1);
}

#[tokio::test]
async fn hands_the_detector_the_safe_head_it_dispatched_against() {
    let mut fixture = plain();
    fixture.reorg_detector.settled_through(HEAD);

    let _ = fixture.indexer.run_once().await;

    assert_eq!(
        fixture.reorg_detector.given_safe_heads(),
        vec![Some(HEAD), Some(HEAD)],
        "inspect, then commit"
    );
}

/// **Processors, then the cursor, then the detector.** The rewind discards
/// the evidence of the abandoned branch, so it waits until the cursor is
/// durable.
#[tokio::test]
async fn unwinds_in_the_order_processors_then_cursor_then_detector() {
    let mut fixture = plain();
    fixture.reorg_detector.queue([Verdict::Reorg {
        first_invalid_block: 98,
        last_invalid_block: 100,
        last_valid_hash: hash_of(b'a', 97),
    }]);

    let _ = fixture.indexer.run_once().await;

    let entries = fixture.trace.recorded();
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
    assert_eq!(fixture.reorg_detector.rewound_to(), vec![Some(97)]);
}

/// Nothing is mutated until every processor has accepted the discard.
#[tokio::test]
async fn leaves_the_rewind_unapplied_when_a_processor_holds_the_discard() {
    let mut fixture = plain();
    fixture.reorg_detector.queue([Verdict::Reorg {
        first_invalid_block: 98,
        last_invalid_block: 100,
        last_valid_hash: hash_of(b'a', 97),
    }]);
    fixture.processors[0].queue([retry("busy")]);

    let _ = fixture.indexer.run_once().await;

    assert!(fixture.reorg_detector.rewound_to().is_empty());
    assert!(fixture.cursor_store.saved().is_empty());
}

/// The window still holds the run above an unchanged cursor, which is
/// strictly more to work with than the reverse.
#[tokio::test]
async fn keeps_the_retained_headers_when_the_rewound_cursor_is_refused() {
    let mut fixture = plain();
    fixture.reorg_detector.queue([Verdict::Reorg {
        first_invalid_block: 98,
        last_invalid_block: 100,
        last_valid_hash: hash_of(b'a', 97),
    }]);
    fixture.cursor_store.refuse_nth_save(1);

    let _ = fixture.indexer.run_once().await;

    assert!(fixture.reorg_detector.rewound_to().is_empty());
}

#[tokio::test]
async fn stops_when_the_fork_is_beyond_the_detectors_reach() {
    let mut fixture = plain();
    fixture.reorg_detector.queue([Verdict::Unrecoverable {
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
    fixture.reorg_detector.queue([Verdict::Reorg {
        first_invalid_block: 0,
        last_invalid_block: 0,
        last_valid_hash: B256::ZERO,
    }]);

    let result = fixture.indexer.run_once().await;

    assert_eq!(result, Iteration::Progressed { cursor_at: 0 });
    assert_eq!(fixture.reorg_detector.rewound_to(), vec![None]);
    assert!(
        fixture.cursor_store.saved().is_empty(),
        "it saved below genesis"
    );
}
