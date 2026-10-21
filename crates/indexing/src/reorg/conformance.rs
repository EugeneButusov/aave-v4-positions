//! The [`BlockHeaderStore`] port as an executable specification.
//!
//! Every implementation runs it: an adapter that got any of this subtly wrong
//! would *under-report* a reorg rather than fail.

use units::ChainId;

use crate::chain::BlockHeader;
use crate::fixtures::{CHAIN_ID, OTHER_CHAIN, header};
use crate::reorg::BlockHeaderStore;

/// A floor no header can fall below, for the cases retention is not about.
const KEEP_ALL: u64 = 0;

pub(crate) trait Harness {
    type Store: BlockHeaderStore;

    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;
}

async fn window<H: Harness>(harness: &H, chain_id: ChainId) -> Vec<BlockHeader> {
    harness
        .store()
        .load(chain_id)
        .await
        .expect("the window should read")
}

/// **Sorted before comparing.** `load` promises no order, so comparing it as
/// given would pass on insert order and fail the day storage reorganises.
async fn heights<H: Harness>(harness: &H, chain_id: ChainId) -> Vec<u64> {
    let mut numbers: Vec<u64> = window(harness, chain_id)
        .await
        .iter()
        .map(|header| header.number)
        .collect();
    numbers.sort_unstable();

    numbers
}

async fn append<H: Harness>(harness: &H, chain_id: ChainId, block: u64, retain_from: u64) {
    harness
        .store()
        .append(chain_id, &header(block, b'a'), retain_from)
        .await
        .expect("the header should retain");
}

async fn truncate<H: Harness>(harness: &H, chain_id: ChainId, last_valid: Option<u64>) {
    harness
        .store()
        .truncate(chain_id, last_valid)
        .await
        .expect("the window should truncate");
}

pub(crate) async fn answers_an_empty_window_for_a_chain_never_seen<H: Harness>() {
    let harness = H::fresh("answers_an_empty_window_for_a_chain_never_seen").await;

    assert!(window(&harness, CHAIN_ID).await.is_empty());
}

pub(crate) async fn round_trips_a_retained_header<H: Harness>() {
    let harness = H::fresh("round_trips_a_retained_header").await;
    let retained = header(500, b'a');

    harness
        .store()
        .append(CHAIN_ID, &retained, KEEP_ALL)
        .await
        .expect("the header should retain");

    assert_eq!(window(&harness, CHAIN_ID).await, vec![retained]);
}

pub(crate) async fn holds_one_header_per_height<H: Harness>() {
    let harness = H::fresh("holds_one_header_per_height").await;
    let replacement = header(500, b'b');

    append(&harness, CHAIN_ID, 500, KEEP_ALL).await;
    harness
        .store()
        .append(CHAIN_ID, &replacement, KEEP_ALL)
        .await
        .expect("the header should retain");

    assert_eq!(window(&harness, CHAIN_ID).await, vec![replacement]);
}

pub(crate) async fn prunes_below_the_floor_and_keeps_the_floor_itself<H: Harness>() {
    let harness = H::fresh("prunes_below_the_floor_and_keeps_the_floor_itself").await;

    for block in 500..=502 {
        append(&harness, CHAIN_ID, block, KEEP_ALL).await;
    }
    append(&harness, CHAIN_ID, 503, 501).await;

    assert_eq!(heights(&harness, CHAIN_ID).await, vec![501, 502, 503]);
}

pub(crate) async fn prunes_only_the_chain_it_was_given<H: Harness>() {
    // A `DELETE` missing its chain predicate passes every single-chain case.
    let harness = H::fresh("prunes_only_the_chain_it_was_given").await;

    append(&harness, OTHER_CHAIN, 100, KEEP_ALL).await;
    append(&harness, CHAIN_ID, 503, 501).await;

    assert_eq!(heights(&harness, OTHER_CHAIN).await, vec![100]);
}

pub(crate) async fn drops_only_what_sits_above_a_truncation_point<H: Harness>() {
    let harness = H::fresh("drops_only_what_sits_above_a_truncation_point").await;

    for block in 500..=502 {
        append(&harness, CHAIN_ID, block, KEEP_ALL).await;
    }
    truncate(&harness, CHAIN_ID, Some(500)).await;

    assert_eq!(heights(&harness, CHAIN_ID).await, vec![500]);
}

pub(crate) async fn truncates_only_the_chain_it_was_asked_about<H: Harness>() {
    let harness = H::fresh("truncates_only_the_chain_it_was_asked_about").await;

    append(&harness, OTHER_CHAIN, 600, KEEP_ALL).await;
    append(&harness, CHAIN_ID, 600, KEEP_ALL).await;
    truncate(&harness, CHAIN_ID, Some(500)).await;

    assert_eq!(heights(&harness, OTHER_CHAIN).await, vec![600]);
}

pub(crate) async fn clears_the_window_when_truncated_below_everything<H: Harness>() {
    let harness = H::fresh("clears_the_window_when_truncated_below_everything").await;

    append(&harness, CHAIN_ID, 0, KEEP_ALL).await;
    append(&harness, CHAIN_ID, 500, KEEP_ALL).await;
    truncate(&harness, CHAIN_ID, None).await;

    assert!(heights(&harness, CHAIN_ID).await.is_empty());
}

pub(crate) async fn truncates_a_chain_never_seen_without_complaint<H: Harness>() {
    // Bootstrap clears the window before it knows whether one exists.
    let harness = H::fresh("truncates_a_chain_never_seen_without_complaint").await;

    truncate(&harness, CHAIN_ID, None).await;

    assert!(heights(&harness, CHAIN_ID).await.is_empty());
}

pub(crate) async fn keeps_chains_apart_on_read<H: Harness>() {
    let harness = H::fresh("keeps_chains_apart_on_read").await;

    append(&harness, OTHER_CHAIN, 700, KEEP_ALL).await;

    assert!(heights(&harness, CHAIN_ID).await.is_empty());
}

pub(crate) async fn returns_every_height_written_whatever_the_order<H: Harness>() {
    let harness = H::fresh("returns_every_height_written_whatever_the_order").await;

    for block in [503, 500, 502, 501] {
        append(&harness, CHAIN_ID, block, KEEP_ALL).await;
    }

    assert_eq!(heights(&harness, CHAIN_ID).await, vec![500, 501, 502, 503]);
}

pub(crate) async fn hands_out_a_window_the_caller_may_keep<H: Harness>() {
    // A view over the adapter's own storage would have the detector mutating
    // the window it is deciding from.
    let harness = H::fresh("hands_out_a_window_the_caller_may_keep").await;

    append(&harness, CHAIN_ID, 500, KEEP_ALL).await;
    let mut held = window(&harness, CHAIN_ID).await;
    held.clear();

    assert_eq!(heights(&harness, CHAIN_ID).await, vec![500]);
}

macro_rules! block_header_store_conformance {
    ($harness:ty) => {
        $crate::reorg::conformance::block_header_store_conformance!(@cases $harness:
            answers_an_empty_window_for_a_chain_never_seen
            round_trips_a_retained_header
            holds_one_header_per_height
            prunes_below_the_floor_and_keeps_the_floor_itself
            prunes_only_the_chain_it_was_given
            drops_only_what_sits_above_a_truncation_point
            truncates_only_the_chain_it_was_asked_about
            clears_the_window_when_truncated_below_everything
            truncates_a_chain_never_seen_without_complaint
            keeps_chains_apart_on_read
            returns_every_height_written_whatever_the_order
            hands_out_a_window_the_caller_may_keep
        );
    };
    (@cases $harness:ty: $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                $crate::reorg::conformance::$case::<$harness>().await;
            }
        )*
    };
}

pub(crate) use block_header_store_conformance;
