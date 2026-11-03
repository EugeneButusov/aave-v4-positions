use crate::reorg::ReorgDetector;
use crate::reorg::harness::{DEPTH, harness};

#[tokio::test]
async fn settles_one_finality_depth_below_the_observed_head() {
    let fixture = harness(DEPTH).await;

    let settled = fixture.detector.safe_head(500);

    assert_eq!(settled, Some(490));
}

#[tokio::test]
async fn settles_nothing_on_a_chain_shorter_than_the_depth() {
    let fixture = harness(DEPTH).await;

    assert_eq!(fixture.detector.safe_head(DEPTH - 1), None);
    assert_eq!(fixture.detector.safe_head(DEPTH), Some(0));
}
