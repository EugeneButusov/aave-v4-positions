use crate::fixtures::{CHAIN_ID, hash_of};
use crate::reorg::harness::{DEPTH, HEAD, harness};
use crate::reorg::{BlockHeaderStore as _, ReorgDetector};

#[tokio::test]
async fn leaves_no_gap_however_the_ranges_land() {
    let fixture = harness(DEPTH).await;

    fixture.commit([HEAD - 40, HEAD - 20, HEAD - 1, HEAD]).await;

    let retained = fixture.retained().await;
    let contiguous = retained.windows(2).all(|pair| pair[1] == pair[0] + 1);

    assert!(contiguous, "{retained:?}");
}

// The extra header anchors a fork immediately above the safe head.
#[tokio::test]
async fn retains_one_more_than_the_depth() {
    let fixture = harness(DEPTH).await;

    fixture.commit([HEAD]).await;

    assert_eq!(
        u64::try_from(fixture.retained().await.len()).unwrap(),
        DEPTH + 1
    );
}

#[tokio::test]
async fn pushes_the_oldest_out_as_the_tip_advances() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 1]).await;
    let before = fixture.retained().await;

    fixture.commit([HEAD]).await;
    let after = fixture.retained().await;

    assert_eq!(after.len(), before.len());
    assert_eq!(after.first(), Some(&(before[0] + 1)));
}

#[tokio::test]
async fn keeps_only_the_anchor_for_a_settled_range() {
    let fixture = harness(DEPTH).await;

    fixture.commit([HEAD - 500]).await;

    assert_eq!(fixture.retained().await, vec![HEAD - 500]);
}

#[tokio::test]
async fn fills_the_run_for_the_range_that_lands_on_the_boundary() {
    let fixture = harness(DEPTH).await;

    fixture.commit([HEAD - DEPTH]).await;

    assert_eq!(
        u64::try_from(fixture.retained().await.len()).unwrap(),
        DEPTH + 1
    );
}

#[tokio::test]
async fn tops_the_window_back_up_after_a_rewind() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD]).await;

    fixture.detector.rewind_to(Some(HEAD - 5)).await.unwrap();
    fixture.commit([HEAD - 4]).await;

    assert_eq!(
        u64::try_from(fixture.retained().await.len()).unwrap(),
        DEPTH + 1
    );
}

#[tokio::test]
async fn keeps_the_block_a_rewind_landed_on() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD]).await;

    fixture.detector.rewind_to(Some(HEAD - 5)).await.unwrap();

    assert_eq!(fixture.retained().await.last(), Some(&(HEAD - 5)));
}

#[tokio::test]
async fn clears_the_window_when_rewound_below_everything() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD]).await;

    fixture.detector.rewind_to(None).await.unwrap();

    assert!(fixture.retained().await.is_empty());
}

#[tokio::test]
async fn restarts_the_window_when_a_header_does_not_extend_the_run() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 2, HEAD - 1]).await;

    let stacked = crate::fixtures::header(HEAD, b'b');
    fixture
        .detector
        .commit(&stacked, fixture.safe_head)
        .await
        .unwrap();

    let retained = fixture.retained().await;
    let window = fixture.block_header_store.load(CHAIN_ID).await.unwrap();

    assert!(
        window
            .iter()
            .all(|held| held.hash == hash_of(b'b', held.number)),
        "a branch-a header survived the restart: {retained:?}"
    );
}

#[tokio::test]
async fn replaces_a_height_re_committed_on_another_branch() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 1]).await;
    fixture.chain_client.fork_above(HEAD - 2, b'b');

    fixture.commit([HEAD - 1]).await;
    let window = fixture.block_header_store.load(CHAIN_ID).await.unwrap();

    let at_height: Vec<_> = window
        .iter()
        .filter(|held| held.number == HEAD - 1)
        .collect();

    assert_eq!(at_height.len(), 1, "{window:?}");
    assert_eq!(at_height[0].hash, hash_of(b'b', HEAD - 1));
}
