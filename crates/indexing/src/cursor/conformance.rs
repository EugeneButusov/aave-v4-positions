//! The [`SyncStatusStore`] port as an executable specification.
//!
//! Every implementation runs it. The port is read-only — the loop that writes
//! this row arrives in Phase 3 — so an implementation supplies a [`Harness`]
//! saying how state gets in front of it.

use alloy_primitives::{B256, b256};
use units::{ChainId, UnixSeconds};

use crate::cursor::{SyncStatus, SyncStatusStore};

pub(crate) const CHAIN_ID: ChainId = ChainId::known(1);
pub(crate) const OTHER_CHAIN: ChainId = ChainId::known(8453);

pub(crate) const HEAD: B256 =
    b256!("abababababababababababababababababababababababababababababababab");

/// One row of `indexer_cursor`, as a case wants to describe it.
pub(crate) struct Advanced {
    pub chain_id: ChainId,
    pub last_block: u64,
    pub last_hash: B256,
    /// How far in the past the database should stamp `updated_at`.
    pub aged_seconds: u64,
}

impl Advanced {
    /// Every field, named once. `aged_seconds` is how far in the past the
    /// database should stamp `updated_at`.
    pub(crate) fn aged(chain_id: ChainId, last_block: u64, aged_seconds: u64) -> Self {
        Self {
            chain_id,
            last_block,
            last_hash: HEAD,
            aged_seconds,
        }
    }

    /// The same row, written just now.
    pub(crate) fn to(chain_id: ChainId, last_block: u64) -> Self {
        Self::aged(chain_id, last_block, 0)
    }
}

/// One row of `indexer_block_headers`, as a case wants to describe it. Only the
/// three columns the read joins on and reads.
pub(crate) struct Header {
    pub chain_id: ChainId,
    pub block_number: u64,
    pub block_timestamp: UnixSeconds,
}

impl Header {
    pub(crate) fn at(chain_id: ChainId, block_number: u64, block_timestamp: u64) -> Self {
        Self {
            chain_id,
            block_number,
            block_timestamp: UnixSeconds::new(block_timestamp),
        }
    }
}

pub(crate) trait Harness {
    type Store: SyncStatusStore;

    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;

    async fn given_cursor(&self, rows: &[Advanced]);

    async fn given_headers(&self, rows: &[Header]);
}

async fn status_of<H: Harness>(harness: &H) -> Option<SyncStatus> {
    harness
        .store()
        .get(CHAIN_ID)
        .await
        .expect("the status should read")
}

pub(crate) async fn reads_the_position_the_indexer_left<H: Harness>() {
    let harness = H::fresh("reads_the_position_the_indexer_left").await;
    harness
        .given_cursor(&[Advanced::to(CHAIN_ID, 25_652_535)])
        .await;

    let status = status_of(&harness).await.expect("the chain is indexed");

    assert_eq!(status.chain_id, CHAIN_ID);
    assert_eq!(status.last_block, 25_652_535);
    assert_eq!(status.last_hash, HEAD);
}

pub(crate) async fn answers_none_for_a_chain_never_indexed<H: Harness>() {
    // Which the read path turns into a 404 rather than an empty page: a chain
    // this deployment does not serve is a different claim from a wallet that
    // holds nothing.
    let harness = H::fresh("answers_none_for_a_chain_never_indexed").await;

    assert!(status_of(&harness).await.is_none());
}

pub(crate) async fn reads_only_the_chain_it_was_asked_about<H: Harness>() {
    let harness = H::fresh("reads_only_the_chain_it_was_asked_about").await;
    harness
        .given_cursor(&[Advanced::to(OTHER_CHAIN, 999)])
        .await;

    assert!(status_of(&harness).await.is_none(), "another chain's row");
}

pub(crate) async fn ages_the_row_by_the_database_clock<H: Harness>() {
    let harness = H::fresh("ages_the_row_by_the_database_clock").await;
    harness
        .given_cursor(&[Advanced::aged(CHAIN_ID, 100, 90)])
        .await;

    let age = status_of(&harness).await.expect("indexed").age_seconds;

    assert!((90..95).contains(&age), "{age}");
}

pub(crate) async fn reads_a_block_height_past_what_a_double_can_hold<H: Harness>() {
    // `bigint` holds it and a JSON number does not. Nothing reaches this height
    // today; the point is that the column's width is honoured rather than
    // narrowed on the way through.
    const HIGH: u64 = 9_007_199_254_740_993;

    let harness = H::fresh("reads_a_block_height_past_what_a_double_can_hold").await;
    harness.given_cursor(&[Advanced::to(CHAIN_ID, HIGH)]).await;

    assert_eq!(status_of(&harness).await.expect("indexed").last_block, HIGH);
}

/// 2026-07-25T17:20:00Z, as a block would report it.
const MINED_AT: u64 = 1_785_000_000;

pub(crate) async fn reads_the_timestamp_of_the_block_the_cursor_names<H: Harness>() {
    let harness = H::fresh("reads_the_timestamp_of_the_block_the_cursor_names").await;
    harness.given_cursor(&[Advanced::to(CHAIN_ID, 100)]).await;
    harness
        .given_headers(&[
            Header::at(CHAIN_ID, 99, MINED_AT - 12),
            Header::at(CHAIN_ID, 100, MINED_AT),
        ])
        .await;

    let status = status_of(&harness).await.expect("indexed");

    assert_eq!(
        status.last_block_timestamp,
        Some(UnixSeconds::new(MINED_AT)),
        "the header at the height the cursor names, not the newest one"
    );
}

pub(crate) async fn answers_no_timestamp_when_the_window_has_no_header_for_that_height<
    H: Harness,
>() {
    // Which the window genuinely passes through: `commit` truncates, appends the
    // new top and then walks down filling the run, and the cursor is saved after
    // all of it. The read must not substitute its own clock for the gap.
    let harness =
        H::fresh("answers_no_timestamp_when_the_window_has_no_header_for_that_height").await;
    harness.given_cursor(&[Advanced::to(CHAIN_ID, 100)]).await;
    harness
        .given_headers(&[Header::at(CHAIN_ID, 99, MINED_AT - 12)])
        .await;

    let status = status_of(&harness).await.expect("indexed");

    assert_eq!(status.last_block, 100);
    assert_eq!(status.last_block_timestamp, None);
}

pub(crate) async fn takes_the_header_of_the_chain_it_was_asked_about<H: Harness>() {
    let harness = H::fresh("takes_the_header_of_the_chain_it_was_asked_about").await;
    harness.given_cursor(&[Advanced::to(CHAIN_ID, 100)]).await;
    harness
        .given_headers(&[
            Header::at(OTHER_CHAIN, 100, MINED_AT - 3_600),
            Header::at(CHAIN_ID, 100, MINED_AT),
        ])
        .await;

    let status = status_of(&harness).await.expect("indexed");

    assert_eq!(
        status.last_block_timestamp,
        Some(UnixSeconds::new(MINED_AT)),
        "another chain's block at the same height is a different block"
    );
}
