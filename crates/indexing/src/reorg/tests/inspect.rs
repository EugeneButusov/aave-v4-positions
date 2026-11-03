use crate::reorg::harness::{DEPTH, HEAD, at_head, harness, reorg, unrecoverable};
use crate::reorg::{Error, ReorgDetector, Verdict};

#[tokio::test]
async fn accepts_a_header_whose_parent_is_retained_without_reading_the_chain() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 1]).await;
    let before = fixture.chain_client.header_reads();

    let verdict = fixture
        .detector
        .inspect(&fixture.header(HEAD), fixture.safe_head)
        .await
        .unwrap();

    assert_eq!(verdict, Verdict::Continuous);
    assert_eq!(
        fixture.chain_client.header_reads(),
        before,
        "it read the chain"
    );
}

#[tokio::test]
async fn accepts_the_first_block_with_an_empty_window() {
    let fixture = harness(DEPTH).await;

    let verdict = fixture
        .detector
        .inspect(&fixture.header(HEAD), fixture.safe_head)
        .await
        .unwrap();

    assert_eq!(verdict, Verdict::Continuous);
}

#[tokio::test]
async fn waves_a_settled_block_through_without_reading_the_window() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 1]).await;
    let before = fixture.chain_client.header_reads();

    let settled = crate::fixtures::header(HEAD - DEPTH - 5, b'z');
    let verdict = fixture
        .detector
        .inspect(&settled, fixture.safe_head)
        .await
        .unwrap();

    assert_eq!(verdict, Verdict::Continuous);
    assert_eq!(
        fixture.chain_client.header_reads(),
        before,
        "it read the chain"
    );
}

#[tokio::test]
async fn names_the_invalid_range_when_a_parent_hash_does_not_match() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 3, HEAD - 2, HEAD - 1]).await;
    fixture.chain_client.fork_above(HEAD - 3, b'b');

    let verdict = fixture
        .detector
        .inspect(&fixture.header(HEAD), fixture.safe_head)
        .await
        .unwrap();

    assert_eq!(reorg(&verdict), (HEAD - 2, HEAD - 1));
}

#[tokio::test]
async fn finds_a_one_block_fork_at_the_tip() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 2, HEAD - 1]).await;
    fixture.chain_client.fork_above(HEAD - 2, b'b');

    let verdict = fixture
        .detector
        .inspect(&fixture.header(HEAD), fixture.safe_head)
        .await
        .unwrap();

    assert_eq!(reorg(&verdict), (HEAD - 1, HEAD - 1));
}

#[tokio::test]
async fn catches_a_swap_at_a_height_committed_but_never_cursored() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 2, HEAD - 1]).await;
    fixture.chain_client.fork_above(HEAD - 2, b'b');

    let replacement = fixture.header(HEAD - 1);
    let verdict = fixture
        .detector
        .inspect(&replacement, fixture.safe_head)
        .await
        .unwrap();

    assert_eq!(reorg(&verdict), (HEAD - 1, HEAD - 1));
}

#[tokio::test]
async fn tolerates_re_inspecting_a_block_it_already_holds() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 1, HEAD]).await;

    let verdict = fixture
        .detector
        .inspect(&fixture.header(HEAD), fixture.safe_head)
        .await
        .unwrap();

    assert_eq!(verdict, Verdict::Continuous);
}

#[tokio::test]
async fn refuses_when_nothing_is_retained_for_the_block_beneath() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 5]).await;

    let verdict = fixture
        .detector
        .inspect(&fixture.header(HEAD), fixture.safe_head)
        .await
        .unwrap();

    assert!(
        unrecoverable(&verdict).contains(&format!("nothing retained for block {}", HEAD - 1)),
        "{verdict:?}"
    );
}

#[tokio::test]
async fn refuses_when_no_retained_header_is_still_canonical() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 2, HEAD - 1]).await;

    fixture.chain_client.fork_above(HEAD - 2 - DEPTH, b'b');

    let verdict = fixture
        .detector
        .inspect(&fixture.header(HEAD), fixture.safe_head)
        .await
        .unwrap();

    assert!(
        unrecoverable(&verdict).contains("is still canonical"),
        "{verdict:?}"
    );
}

#[tokio::test]
async fn surfaces_a_failed_read_rather_than_calling_it_a_fork() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 2, HEAD - 1]).await;
    fixture.chain_client.fork_above(HEAD - 2, b'b');
    fixture.chain_client.fail_next(1);

    let refusal = fixture
        .detector
        .inspect(&fixture.header(HEAD), fixture.safe_head)
        .await
        .expect_err("the walk needs the chain");

    assert!(matches!(refusal, Error::ChainReadFailed(_)), "{refusal:?}");
}

#[tokio::test]
async fn refuses_a_swap_at_genesis_with_nothing_below_it() {
    let fixture = at_head(1, 0).await;
    fixture.commit([0]).await;

    let replaced = crate::fixtures::header(0, b'b');
    let verdict = fixture
        .detector
        .inspect(&replaced, fixture.safe_head)
        .await
        .unwrap();

    assert!(
        unrecoverable(&verdict).contains("nothing below block 0"),
        "{verdict:?}"
    );
}
