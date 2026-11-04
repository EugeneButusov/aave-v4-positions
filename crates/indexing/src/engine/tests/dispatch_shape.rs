use crate::engine::harness::{HEAD, MAX_RANGE, START, plain};
use crate::fixtures::hash_of;
use crate::reorg::Verdict;

#[tokio::test]
async fn dispatches_a_full_width_range_while_catching_up() {
    let mut fixture = plain();
    fixture.reorg_detector.settled_through(HEAD);

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
    fixture.reorg_detector.settled_through(START + 10);

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
    assert!(fixture.reorg_detector.rewound_to().is_empty());
}

/// The replacement range comes on the next iteration.
#[tokio::test]
async fn blocks_and_a_reorg_never_go_out_in_one_iteration() {
    let mut fixture = plain();
    fixture.reorg_detector.queue([Verdict::Reorg {
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
