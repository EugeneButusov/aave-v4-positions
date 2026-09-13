//! The [`SyncStatusStore`] port as an executable specification.
//!
//! Every implementation runs it. The port is read-only — the loop that writes
//! this row arrives in Phase 3 — so an implementation supplies a [`Fixture`]
//! saying how state gets in front of it.

use alloy_primitives::{B256, b256};

use crate::cursor::{SyncStatus, SyncStatusStore};

pub(crate) const CHAIN_ID: u32 = 1;
pub(crate) const OTHER_CHAIN: u32 = 8453;

pub(crate) const HEAD: B256 =
    b256!("abababababababababababababababababababababababababababababababab");

/// One row of `indexer_cursor`, as a case wants to describe it.
pub(crate) struct Advanced {
    pub chain_id: u32,
    pub last_block: u64,
    pub last_hash: B256,
    /// How far in the past the database should stamp `updated_at`.
    pub aged_seconds: u64,
}

impl Advanced {
    /// Every field, named once. `aged_seconds` is how far in the past the
    /// database should stamp `updated_at`.
    pub(crate) fn aged(chain_id: u32, last_block: u64, aged_seconds: u64) -> Self {
        Self {
            chain_id,
            last_block,
            last_hash: HEAD,
            aged_seconds,
        }
    }

    /// The same row, written just now.
    pub(crate) fn to(chain_id: u32, last_block: u64) -> Self {
        Self::aged(chain_id, last_block, 0)
    }
}

pub(crate) trait Fixture {
    type Store: SyncStatusStore;

    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;

    async fn given_cursor(&self, rows: &[Advanced]);
}

async fn status_of<F: Fixture>(fixture: &F) -> Option<SyncStatus> {
    fixture
        .store()
        .get(CHAIN_ID)
        .await
        .expect("the status should read")
}

pub(crate) async fn reads_the_position_the_indexer_left<F: Fixture>() {
    let fixture = F::fresh("reads_the_position_the_indexer_left").await;
    fixture
        .given_cursor(&[Advanced::to(CHAIN_ID, 25_652_535)])
        .await;

    let status = status_of(&fixture).await.expect("the chain is indexed");

    assert_eq!(status.chain_id, CHAIN_ID);
    assert_eq!(status.last_block, 25_652_535);
    assert_eq!(status.last_hash, HEAD);
}

pub(crate) async fn answers_none_for_a_chain_never_indexed<F: Fixture>() {
    // Which the read path turns into a 404 rather than an empty page: a chain
    // this deployment does not serve is a different claim from a wallet that
    // holds nothing.
    let fixture = F::fresh("answers_none_for_a_chain_never_indexed").await;

    assert!(status_of(&fixture).await.is_none());
}

pub(crate) async fn reads_only_the_chain_it_was_asked_about<F: Fixture>() {
    let fixture = F::fresh("reads_only_the_chain_it_was_asked_about").await;
    fixture
        .given_cursor(&[Advanced::to(OTHER_CHAIN, 999)])
        .await;

    assert!(status_of(&fixture).await.is_none(), "another chain's row");
}

pub(crate) async fn ages_the_row_by_the_database_clock<F: Fixture>() {
    let fixture = F::fresh("ages_the_row_by_the_database_clock").await;
    fixture
        .given_cursor(&[Advanced::aged(CHAIN_ID, 100, 90)])
        .await;

    let age = status_of(&fixture).await.expect("indexed").age_seconds;

    assert!((90..95).contains(&age), "{age}");
}

pub(crate) async fn reads_a_block_height_past_what_a_double_can_hold<F: Fixture>() {
    // `bigint` holds it and a JSON number does not. Nothing reaches this height
    // today; the point is that the column's width is honoured rather than
    // narrowed on the way through.
    const HIGH: u64 = 9_007_199_254_740_993;

    let fixture = F::fresh("reads_a_block_height_past_what_a_double_can_hold").await;
    fixture.given_cursor(&[Advanced::to(CHAIN_ID, HIGH)]).await;

    assert_eq!(status_of(&fixture).await.expect("indexed").last_block, HIGH);
}
