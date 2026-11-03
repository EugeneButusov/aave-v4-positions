use crate::chain::ChainCall;
use crate::cursor::Cursor;
use crate::engine::Iteration;
use crate::engine::harness::{START, Setup, fixture, plain, reason, retry};
use crate::fixtures::{CHAIN_ID, hash_of};
use crate::reorg::Verdict;

#[tokio::test]
async fn bootstraps_once_and_before_reading_the_head() {
    let mut fixture = plain();

    let _ = fixture.indexer.run_once().await;
    let _ = fixture.indexer.run_once().await;

    assert_eq!(fixture.reorg_detector.bootstrapped_with().len(), 1);

    let calls = fixture.chain_client.calls();
    let bootstrap = fixture
        .trace
        .recorded()
        .iter()
        .position(|entry| entry == "detector.bootstrap")
        .expect("it bootstrapped");
    assert_eq!(bootstrap, 0, "{:?}", fixture.trace.recorded());
    assert!(matches!(calls.first(), Some(ChainCall::ChainId)));
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
    assert_eq!(fixture.reorg_detector.bootstrapped_with(), vec![None]);
}

/// The chain forked while the process was down. Nothing else would notice:
/// the window starts empty, so without this the loop would carry on from
/// the cursor onto a branch that lost.
#[tokio::test]
async fn unwinds_a_fork_reported_at_bootstrap() {
    let mut fixture = plain();
    fixture.reorg_detector.on_bootstrap(Verdict::Reorg {
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
    assert_eq!(fixture.cursor_store.saved().len(), 1);
    assert_eq!(fixture.reorg_detector.rewound_to(), vec![Some(497)]);
}

#[tokio::test]
async fn refuses_to_resume_when_the_detector_cannot_place_the_fork() {
    let mut fixture = plain();
    fixture.reorg_detector.on_bootstrap(Verdict::Unrecoverable {
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
    fixture.reorg_detector.on_bootstrap(Verdict::Reorg {
        first_invalid_block: 498,
        last_invalid_block: 500,
        last_valid_hash: hash_of(b'a', 497),
    });
    fixture.processors[0].queue([retry("busy")]);

    let _ = fixture.indexer.run_once().await;
    let _ = fixture.indexer.run_once().await;

    assert_eq!(fixture.reorg_detector.bootstrapped_with().len(), 2);
}
