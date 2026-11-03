use crate::reorg::harness::{DEPTH, HEAD, cursor_at, harness, reorg};
use crate::reorg::{ReorgDetector, Verdict};

#[tokio::test]
async fn reports_the_same_fork_again_when_nothing_was_applied() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 3, HEAD - 2, HEAD - 1]).await;
    fixture.chain_client.fork_above(HEAD - 3, b'b');

    let first = fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
        .await
        .unwrap();
    let again = fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
        .await
        .unwrap();

    assert_eq!(first, again);
    assert_eq!(reorg(&again), (HEAD - 2, HEAD - 1));
}

#[tokio::test]
async fn reports_it_again_when_the_cursor_landed_but_the_rewind_did_not() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 3, HEAD - 2, HEAD - 1]).await;
    fixture.chain_client.fork_above(HEAD - 3, b'b');

    let verdict = fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
        .await
        .unwrap();
    let (first_invalid, _) = reorg(&verdict);

    let again = fixture
        .detector
        .bootstrap(Some(&cursor_at(first_invalid - 1, b'a')))
        .await
        .unwrap();

    assert_eq!(reorg(&again), reorg(&verdict));
}

#[tokio::test]
async fn stops_reporting_once_the_rewind_also_ran() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 3, HEAD - 2, HEAD - 1]).await;
    fixture.chain_client.fork_above(HEAD - 3, b'b');

    let verdict = fixture
        .detector
        .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
        .await
        .unwrap();
    let (first_invalid, _) = reorg(&verdict);

    fixture
        .detector
        .rewind_to(Some(first_invalid - 1))
        .await
        .unwrap();

    let after = fixture
        .detector
        .bootstrap(Some(&cursor_at(first_invalid - 1, b'a')))
        .await
        .unwrap();

    assert_eq!(after, Verdict::Continuous);
}
