//! Both of this module's ports as executable specifications.
//!
//! Every implementation runs them. [`SyncStatusStore`] cannot put a row in
//! front of itself, so its [`Harness`] says how state gets there;
//! [`CursorStore`] writes, so [`CursorHarness`] supplies only a store.

use alloy_primitives::{B256, b256};

use crate::cursor::{Cursor, CursorStore, SyncStatus, SyncStatusStore};
use crate::fixtures::{CHAIN_ID, OTHER_CHAIN, hash_of};

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

pub(crate) trait Harness {
    type Store: SyncStatusStore;

    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;

    async fn given_cursor(&self, rows: &[Advanced]);
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

pub(crate) trait CursorHarness {
    type Store: CursorStore;

    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;
}

fn at(chain_id: u32, last_block: u64, branch: u8) -> Cursor {
    Cursor {
        chain_id,
        last_block,
        last_hash: hash_of(branch, last_block),
    }
}

async fn loaded<H: CursorHarness>(harness: &H, chain_id: u32) -> Option<Cursor> {
    harness
        .store()
        .load(chain_id)
        .await
        .expect("the cursor should read")
}

async fn save<H: CursorHarness>(harness: &H, cursor: Cursor) {
    harness
        .store()
        .save(&cursor)
        .await
        .expect("the cursor should save");
}

pub(crate) async fn answers_none_before_anything_was_indexed<H: CursorHarness>() {
    let harness = H::fresh("answers_none_before_anything_was_indexed").await;

    assert!(loaded(&harness, CHAIN_ID).await.is_none());
}

pub(crate) async fn round_trips_the_position_it_was_given<H: CursorHarness>() {
    let harness = H::fresh("round_trips_the_position_it_was_given").await;
    let saved = at(CHAIN_ID, 25_652_535, b'a');

    save(&harness, saved).await;

    assert_eq!(loaded(&harness, CHAIN_ID).await, Some(saved));
}

pub(crate) async fn overwrites_rather_than_accumulating<H: CursorHarness>() {
    let harness = H::fresh("overwrites_rather_than_accumulating").await;

    save(&harness, at(CHAIN_ID, 500, b'a')).await;
    save(&harness, at(CHAIN_ID, 501, b'a')).await;

    assert_eq!(
        loaded(&harness, CHAIN_ID).await,
        Some(at(CHAIN_ID, 501, b'a'))
    );
}

pub(crate) async fn accepts_a_cursor_that_moves_backwards<H: CursorHarness>() {
    // Unwinding a fork saves the last *valid* block, which is below where the
    // cursor stood. The guard a reviewer reaches for — `WHERE last_block <
    // EXCLUDED.last_block` — accepts this write, reports success, and silently
    // discards the rewind.
    let harness = H::fresh("accepts_a_cursor_that_moves_backwards").await;
    let unwound = at(CHAIN_ID, 499, b'b');

    save(&harness, at(CHAIN_ID, 500, b'a')).await;
    save(&harness, unwound).await;

    assert_eq!(loaded(&harness, CHAIN_ID).await, Some(unwound));
}

pub(crate) async fn keeps_one_chain_clear_of_another<H: CursorHarness>() {
    let harness = H::fresh("keeps_one_chain_clear_of_another").await;

    save(&harness, at(OTHER_CHAIN, 999, b'a')).await;

    assert!(
        loaded(&harness, CHAIN_ID).await.is_none(),
        "another chain's row"
    );
}

macro_rules! cursor_store_conformance {
    ($harness:ty) => {
        $crate::cursor::conformance::cursor_store_conformance!(@cases $harness:
            answers_none_before_anything_was_indexed
            round_trips_the_position_it_was_given
            overwrites_rather_than_accumulating
            accepts_a_cursor_that_moves_backwards
            keeps_one_chain_clear_of_another
        );
    };
    (@cases $harness:ty: $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                $crate::cursor::conformance::$case::<$harness>().await;
            }
        )*
    };
}

pub(crate) use cursor_store_conformance;
