//! The [`PositionStore`] port as an executable specification.
//!
//! Every implementation runs it. What the port cannot supply is the rows
//! themselves — it is read-only by construction — so an implementation also
//! supplies a [`Harness`] saying how state gets in front of it.
//!
//! Four neighbouring cases stay with the adapter rather than coming here: each
//! observes the fold through the store, and a [`Held`] has already lost what
//! they test — the checksummed spelling, the `caller`, the deltas behind a
//! balance, and what a `LEFT JOIN` miss fills a column with.

use alloy_primitives::{Address, I256, U256};
use units::UnixSeconds;

use super::fixtures::{ALICE, BOB, FIVE_PERCENT, RAY, SECOND_SPOKE, SPOKE, USDC, ask, reserve_ids};
use super::{PositionKey, PositionPage, PositionQuery, PositionStore};

pub(crate) struct Held {
    pub user: Address,
    pub spoke: Address,
    pub reserve_id: &'static str,
    pub supplied_shares: &'static str,
    pub drawn_shares: &'static str,
}

impl Held {
    /// [`ALICE`] on [`SPOKE`], supplying and borrowing nothing.
    fn by(user: Address, reserve_id: &'static str) -> Self {
        Self {
            user,
            spoke: SPOKE,
            reserve_id,
            supplied_shares: "0",
            drawn_shares: "0",
        }
    }

    fn supplying(reserve_id: &'static str, shares: &'static str) -> Self {
        Self {
            supplied_shares: shares,
            ..Self::by(ALICE, reserve_id)
        }
    }
}

/// A reserve resolved all the way to a token, with a checkpoint to value at.
///
/// 1e6 shares against 1e6 of underlying at 5% a year, so an amount is its
/// share count at the checkpoint — the arithmetic is `valuation`'s to prove.
pub(crate) struct Listed {
    pub spoke: Address,
    pub reserve_id: &'static str,
    pub asset_id: &'static str,
    pub underlying: Address,
    pub decimals: u8,
    pub checkpoint_at: u64,
}

pub(crate) trait Harness {
    type Store: PositionStore;

    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;

    /// Positions opened in the same block as the checkpoint every case values
    /// from, which is what most of them want.
    async fn given_positions(&self, held: &[Held]) {
        self.given_positions_at(CHECKPOINT_BLOCK, held).await;
    }

    /// Positions opened in a block of the case's choosing, so a cut can fall
    /// between two of them and the fold has something to rewind.
    async fn given_positions_at(&self, block: u64, held: &[Held]);

    async fn given_reserve(&self, listed: &Listed);

    /// A second `UpdateAsset` on a listed reserve, so a case can ask which
    /// checkpoint a valuation started from.
    async fn given_checkpoint(&self, asset_id: &str, at: u64, drawn_index: &str);
}

/// Deliberately out of numeric order as text: 13 sorts before 3.
const RESERVES: [&str; 5] = ["3", "7", "13", "21", "34"];

pub(crate) const CHECKPOINT_BLOCK: u64 = 100;
pub(crate) const CHECKPOINT_AT: u64 = 1_785_000_100;
pub(crate) const YEAR: u64 = 365 * 24 * 3600;

fn listed(reserve_id: &'static str, asset_id: &'static str) -> Listed {
    Listed {
        spoke: SPOKE,
        reserve_id,
        asset_id,
        underlying: USDC,
        decimals: 6,
        checkpoint_at: CHECKPOINT_AT,
    }
}

pub(crate) async fn returns_one_wallet_on_one_spoke_and_nobody_else<H: Harness>() {
    let harness = H::fresh("one_wallet").await;
    harness
        .given_positions(&[
            Held::supplying("7", "500"),
            Held {
                supplied_shares: "900",
                ..Held::by(BOB, "7")
            },
        ])
        .await;

    // Required rather than optional: with `chain_id` it is the leading pair of
    // the sorting key, so a page is a seek rather than a scan.
    let page = harness.store().list(&ask()).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].user, ALICE);
    assert_eq!(page.items[0].supplied_shares.to_string(), "500");
}

pub(crate) async fn finds_nothing_on_a_spoke_the_wallet_has_never_touched<H: Harness>() {
    let harness = H::fresh("untouched_spoke").await;
    harness
        .given_positions(&[Held::supplying("7", "500")])
        .await;

    let elsewhere = PositionQuery {
        spoke: Some(Address::repeat_byte(0x11)),
        ..ask()
    };
    assert!(
        harness
            .store()
            .list(&elsewhere)
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

pub(crate) async fn keeps_a_debt_only_position_with_no_supply_behind_it<H: Harness>() {
    let harness = H::fresh("debt_only").await;
    harness
        .given_positions(&[Held {
            drawn_shares: "400",
            ..Held::by(ALICE, "13")
        }])
        .await;

    let page = harness.store().list(&ask()).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].reserve_id, U256::from(13));
    assert_eq!(page.items[0].supplied_shares.to_string(), "0");
    assert_eq!(page.items[0].drawn_shares.to_string(), "400");
}

async fn both_spokes<H: Harness>(case: &str) -> H {
    let harness = H::fresh(case).await;
    harness
        .given_positions(&[
            Held::supplying("7", "500"),
            Held {
                spoke: SECOND_SPOKE,
                supplied_shares: "900",
                ..Held::by(ALICE, "7")
            },
        ])
        .await;
    harness
}

pub(crate) async fn lists_every_spoke_when_none_is_named_each_row_saying_which<H: Harness>() {
    let harness = both_spokes::<H>("every_spoke").await;

    // Reserve ids are Spoke-scoped, so nothing may be merged on them. Every
    // row carries its own Spoke so a caller can group rather than guess.
    let page = harness
        .store()
        .list(&PositionQuery {
            spoke: None,
            ..ask()
        })
        .await
        .unwrap();

    let seen: Vec<_> = page
        .items
        .iter()
        .map(|item| (item.spoke, item.supplied_shares.to_string()))
        .collect();
    assert_eq!(
        seen,
        vec![(SPOKE, "500".to_owned()), (SECOND_SPOKE, "900".to_owned())]
    );
}

pub(crate) async fn narrows_to_one_when_it_is_named<H: Harness>() {
    let harness = both_spokes::<H>("one_spoke").await;

    let page = harness
        .store()
        .list(&PositionQuery {
            spoke: Some(SECOND_SPOKE),
            ..ask()
        })
        .await
        .unwrap();

    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].spoke, SECOND_SPOKE);
    assert_eq!(page.items[0].supplied_shares.to_string(), "900");
}

pub(crate) async fn walks_a_page_boundary_that_falls_between_two_spokes<H: Harness>() {
    let harness = both_spokes::<H>("spoke_boundary").await;

    // A `reserve_id`-only key would resume at "> 7" and lose the second
    // Spoke's reserve 7, which sorts after the first Spoke's.
    let first = harness
        .store()
        .list(&PositionQuery {
            spoke: None,
            limit: 1,
            ..ask()
        })
        .await
        .unwrap();
    assert_eq!(
        first.next,
        Some(PositionKey {
            spoke: SPOKE,
            reserve_id: U256::from(7)
        })
    );

    let second = harness
        .store()
        .list(&PositionQuery {
            spoke: None,
            limit: 1,
            after: first.next,
            ..ask()
        })
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.items[0].spoke, SECOND_SPOKE);
    assert_eq!(second.next, None);
}

async fn five_reserves<H: Harness>(case: &str) -> H {
    let harness = H::fresh(case).await;
    let held: Vec<_> = RESERVES
        .iter()
        .map(|reserve_id| Held::supplying(reserve_id, "500"))
        .collect();
    harness.given_positions(&held).await;
    harness
}

pub(crate) async fn walks_every_position_exactly_once_in_numeric_order<H: Harness>() {
    let harness = five_reserves::<H>("walk").await;

    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = harness
            .store()
            .list(&PositionQuery {
                limit: 2,
                after,
                ..ask()
            })
            .await
            .unwrap();
        seen.extend(reserve_ids(&page));
        after = page.next;
        if after.is_none() {
            break;
        }
    }

    // Keyset, so nothing is returned twice or skipped even though the indexer
    // writes between pages. Ordered as UInt256, so 13 precedes 21, not 3.
    assert_eq!(seen, RESERVES);
}

pub(crate) async fn resumes_after_the_row_the_key_names_not_before_it<H: Harness>() {
    let harness = five_reserves::<H>("resume").await;

    let first = harness
        .store()
        .list(&PositionQuery { limit: 2, ..ask() })
        .await
        .unwrap();
    let second = harness
        .store()
        .list(&PositionQuery {
            limit: 2,
            after: first.next.clone(),
            ..ask()
        })
        .await
        .unwrap();

    assert_eq!(reserve_ids(&first), ["3", "7"]);
    assert_eq!(reserve_ids(&second), ["13", "21"]);
}

pub(crate) async fn reports_no_next_key_when_the_page_is_not_full<H: Harness>() {
    let harness = five_reserves::<H>("last_page").await;

    let full = harness
        .store()
        .list(&PositionQuery { limit: 5, ..ask() })
        .await
        .unwrap();
    assert_eq!(full.next, None);

    let short = harness
        .store()
        .list(&PositionQuery { limit: 4, ..ask() })
        .await
        .unwrap();
    assert_eq!(
        short.next,
        Some(PositionKey {
            spoke: SPOKE,
            reserve_id: U256::from(21)
        })
    );
}

pub(crate) async fn values_every_position_on_a_page_at_one_instant<H: Harness>() {
    let harness = H::fresh("one_instant").await;
    harness.given_reserve(&listed("7", "7")).await;
    harness.given_reserve(&listed("13", "13")).await;
    harness
        .given_positions(&[Held::supplying("7", "1000"), Held::supplying("13", "500")])
        .await;

    let page = harness.store().list(&ask()).await.unwrap();

    // Two positions, so one instant for the whole page is evidence rather than
    // a tautology (§12.6). The drawn index they share is the evidence: the page
    // no longer echoes the instant back, so there is nothing to restate.
    assert_eq!(page.items.len(), 2);
    let indexes: std::collections::BTreeSet<_> = page
        .items
        .iter()
        .map(|item| item.value.as_ref().map(|value| value.drawn_index))
        .collect();
    assert_eq!(indexes.len(), 1);
}

/// Every case above, as one `#[tokio::test]` each against one implementation.
///
/// One test per case rather than one per suite, so a failure names the case. A
/// case left out of the list trips `dead_code` rather than going silent.
/// A checkpoint and a supply after the instant these cases ask for: with only
/// one of each, a store ignoring the instant would pass every case above.
const LATER_BLOCK: u64 = CHECKPOINT_BLOCK + 200;
const LATER_AT: u64 = CHECKPOINT_AT + 200;
/// An index no extrapolation from the first checkpoint could reach.
const DOUBLED: &str = "2000000000000000000000000000";

fn decimal(value: &str) -> U256 {
    U256::from_str_radix(value, 10).unwrap()
}

async fn cut_at<H: Harness>(harness: &H, at: u64) -> PositionPage {
    harness
        .store()
        .list(&PositionQuery {
            cut: UnixSeconds::new(at),
            ..ask()
        })
        .await
        .unwrap()
}

async fn two_checkpoints<H: Harness>(case: &str) -> H {
    let harness = H::fresh(case).await;
    harness.given_reserve(&listed("7", "7")).await;
    harness.given_checkpoint("7", LATER_AT, DOUBLED).await;
    // Opened before the first checkpoint, so a case can value earlier than
    // any checkpoint and still have a position.
    harness
        .given_positions_at(CHECKPOINT_BLOCK - 50, &[Held::supplying("7", "1000")])
        .await;
    harness
}

pub(crate) async fn takes_the_checkpoint_in_force_not_the_newest_there_is<H: Harness>() {
    let harness = two_checkpoints::<H>("in_force").await;

    let page = cut_at(&harness, CHECKPOINT_AT).await;

    // The newest checkpoint is 200s ahead, so reaching it is interest over a
    // negative elapsed — which the arithmetic refuses.
    assert_eq!(
        page.items[0].value.as_ref().unwrap().drawn_index,
        decimal(RAY)
    );
}

pub(crate) async fn carries_that_checkpoint_forward_rather_than_snapping_to_it<H: Harness>() {
    let harness = two_checkpoints::<H>("carries_forward").await;

    let page = cut_at(&harness, CHECKPOINT_AT + 100).await;

    // The cut selects a base; it does not replace the extrapolation.
    let interest = decimal(FIVE_PERCENT)
        .checked_mul(U256::from(100))
        .and_then(|accrued| accrued.checked_div(U256::from(YEAR)))
        .unwrap();
    let expected = decimal(RAY).checked_add(interest).unwrap();
    assert_eq!(page.items[0].value.as_ref().unwrap().drawn_index, expected);
}

pub(crate) async fn reports_no_value_when_no_checkpoint_precedes_the_instant<H: Harness>() {
    let harness = two_checkpoints::<H>("before_any").await;

    let page = cut_at(&harness, CHECKPOINT_AT - 1).await;

    // Nothing to carry forward, so no number is offered. The listing is not
    // cut away, so the asset still resolves.
    assert!(page.items[0].value.is_none());
    assert!(page.items[0].asset.is_some());
}

pub(crate) async fn returns_the_shares_held_then_not_the_ones_held_since<H: Harness>() {
    let harness = H::fresh("shares_then").await;
    harness.given_reserve(&listed("7", "7")).await;
    harness
        .given_positions(&[Held::supplying("7", "1000")])
        .await;
    harness
        .given_positions_at(LATER_BLOCK, &[Held::supplying("7", "500")])
        .await;

    // Reading the fold at now would report 1500 at both.
    assert_eq!(
        cut_at(&harness, CHECKPOINT_AT + 100).await.items[0].supplied_shares,
        I256::try_from(1000).unwrap()
    );
    assert_eq!(
        cut_at(&harness, LATER_AT + 100).await.items[0].supplied_shares,
        I256::try_from(1500).unwrap()
    );
}

pub(crate) async fn leaves_out_a_position_that_did_not_exist_yet<H: Harness>() {
    let harness = H::fresh("not_yet").await;
    harness.given_reserve(&listed("7", "7")).await;
    harness
        .given_positions_at(LATER_BLOCK, &[Held::supplying("7", "1000")])
        .await;

    // Its shares sum to nothing before its first event, which the listing
    // filter drops — the rule that hides a closed position.
    assert!(cut_at(&harness, CHECKPOINT_AT + 100).await.items.is_empty());
    assert_eq!(cut_at(&harness, LATER_AT + 100).await.items.len(), 1);
}

macro_rules! position_store_conformance {
    ($harness:ty) => {
        $crate::store::conformance::position_store_conformance!(@cases $harness:
            returns_one_wallet_on_one_spoke_and_nobody_else
            finds_nothing_on_a_spoke_the_wallet_has_never_touched
            keeps_a_debt_only_position_with_no_supply_behind_it
            lists_every_spoke_when_none_is_named_each_row_saying_which
            narrows_to_one_when_it_is_named
            walks_a_page_boundary_that_falls_between_two_spokes
            walks_every_position_exactly_once_in_numeric_order
            resumes_after_the_row_the_key_names_not_before_it
            reports_no_next_key_when_the_page_is_not_full
            values_every_position_on_a_page_at_one_instant
            takes_the_checkpoint_in_force_not_the_newest_there_is
            carries_that_checkpoint_forward_rather_than_snapping_to_it
            reports_no_value_when_no_checkpoint_precedes_the_instant
            returns_the_shares_held_then_not_the_ones_held_since
            leaves_out_a_position_that_did_not_exist_yet
        );
    };
    (@cases $harness:ty: $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                $crate::store::conformance::$case::<$harness>().await;
            }
        )*
    };
}

pub(crate) use position_store_conformance;
