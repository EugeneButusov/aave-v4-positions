use crate::chain::ChainCall;
use crate::reorg::harness::{DEPTH, HEAD, cursor_at, forking, harness, reorg, unrecoverable};
use crate::reorg::{Error, ReorgDetector, Verdict};

#[tokio::test]
async fn rebuilds_the_window_under_a_still_canonical_cursor() {
    let fixture = harness(DEPTH).await;
    let before = fixture.chain_client.header_reads();

    let verdict = fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
        .await
        .unwrap();

    assert_eq!(verdict, Verdict::Continuous);
    assert_eq!(
        fixture.retained().await,
        (HEAD - 1 - DEPTH..=HEAD - 1).collect::<Vec<_>>()
    );
    let depth = usize::try_from(DEPTH).expect("a depth a case chose");
    assert_eq!(fixture.chain_client.header_reads() - before, 1 + depth);
    assert!(
        fixture.chain_client.calls().contains(&ChainCall::Head),
        "{:?}",
        fixture.chain_client.calls()
    );
}

#[tokio::test]
async fn surfaces_a_read_that_fails_while_rebuilding_the_run() {
    let fixture = harness(DEPTH).await;
    fixture.chain_client.fail_next(2);

    let refusal = fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
        .await
        .expect_err("the rebuild needs the chain");

    assert!(matches!(refusal, Error::ChainReadFailed(_)), "{refusal:?}");
}

#[tokio::test]
async fn clears_the_window_when_there_is_no_cursor() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 1]).await;

    let verdict = fixture.detector.bootstrap(None).await.unwrap();

    assert_eq!(verdict, Verdict::Continuous);
    assert!(fixture.retained().await.is_empty());
}

// Commit precedes cursor save, so the durable window may be one block ahead.
#[tokio::test]
async fn vets_the_window_top_rather_than_the_cursor_behind_it() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 2, HEAD - 1]).await;
    fixture.chain_client.fork_above(HEAD - 2, b'b');

    let verdict = fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 2, b'a')))
        .await
        .unwrap();

    assert_eq!(reorg(&verdict), (HEAD - 1, HEAD - 1));
}

#[tokio::test]
async fn continues_when_the_window_is_one_block_ahead_of_the_cursor() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 2, HEAD - 1]).await;

    let verdict = fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 2, b'a')))
        .await
        .unwrap();

    assert_eq!(verdict, Verdict::Continuous);
}

#[tokio::test]
async fn falls_back_to_the_cursor_when_the_window_is_empty() {
    let fixture = harness(DEPTH).await;
    fixture.chain_client.fork_above(HEAD - 5, b'b');

    let verdict = fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
        .await
        .unwrap();

    assert!(
        unrecoverable(&verdict).contains("nothing retained"),
        "{verdict:?}"
    );
}

#[tokio::test]
async fn leaves_the_window_alone_when_the_resume_point_was_reorged_out() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 2, HEAD - 1]).await;
    let before = fixture.retained().await;
    fixture.chain_client.fork_above(HEAD - 2, b'b');

    fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
        .await
        .unwrap();

    assert_eq!(fixture.retained().await, before);
}

#[tokio::test]
async fn stops_rebuilding_where_the_chain_moved_underneath_it() {
    let fixture = forking(DEPTH, 2, HEAD - 5, b'b').await;

    fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
        .await
        .unwrap();

    let retained = fixture.retained().await;

    assert!(
        (u64::try_from(retained.len()).unwrap()) < DEPTH + 1,
        "the run was rebuilt past the point the chain changed: {retained:?}"
    );
}
