use crate::reorg::harness::{DEPTH, HEAD, harness};
use crate::reorg::{Error, ReorgDetector};

#[tokio::test]
async fn a_refused_retention_surfaces_rather_than_being_swallowed() {
    let fixture = harness(DEPTH).await;
    fixture.block_header_store.refuse_nth_retention(1);

    let refusal = fixture
        .detector
        .commit(&fixture.header(HEAD - 1), fixture.safe_head)
        .await
        .expect_err("header retention was refused");

    assert!(matches!(refusal, Error::RetentionRefused), "{refusal:?}");
    assert!(
        fixture.retained().await.is_empty(),
        "it retained a header anyway"
    );
}

#[tokio::test]
async fn a_refused_rewind_leaves_the_reorg_owed() {
    let fixture = harness(DEPTH).await;
    fixture.commit([HEAD - 1]).await;
    fixture.block_header_store.refuse_nth_rewind(1);
    let before = fixture.retained().await;

    let refusal = fixture
        .detector
        .rewind_to(Some(HEAD - 2))
        .await
        .expect_err("the rewind was refused");

    assert!(matches!(refusal, Error::RewindRefused), "{refusal:?}");
    assert_eq!(fixture.retained().await, before);
}
