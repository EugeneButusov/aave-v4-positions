use crate::cursor::Cursor;
use crate::engine::Iteration;
use crate::engine::harness::{HEAD, MAX_RANGE, START, failed, narrowing, plain, reason, retry};
use crate::fixtures::{CHAIN_ID, hash_of};

#[tokio::test]
async fn saves_the_cursor_with_the_hash_of_the_range_top() {
    let mut fixture = plain();
    fixture.reorg_detector.settled_through(HEAD);

    let _ = fixture.indexer.run_once().await;

    let top = START + MAX_RANGE - 1;
    assert_eq!(
        fixture.cursor_store.saved(),
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

    let entries = fixture.trace.recorded();
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
    fixture.reorg_detector.settled_through(HEAD);
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
    fixture.cursor_store.refuse_nth_save(1);

    let first = fixture.indexer.run_once().await;
    let _ = fixture.indexer.run_once().await;

    assert!(matches!(first, Iteration::Retry { .. }), "{first:?}");
    assert_eq!(
        fixture.processors[0].ranges(),
        vec![(START, START), (START, START)]
    );
}
