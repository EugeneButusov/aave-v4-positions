//! The [`PositionStore`] port as an executable specification.
//!
//! Every implementation runs it. What the port cannot supply is the rows
//! themselves — it is read-only by construction — so an implementation also
//! supplies a [`Fixture`] saying how state gets in front of it.
//!
//! **Four of this module's neighbours could not come here**, and they are not
//! an oversight. `matches_a_checksummed_address_against_the_lower_cased_fold`,
//! `credits_the_user_never_the_caller_that_routed_for_them`,
//! `hides_a_position_whose_shares_have_netted_to_zero` and
//! `reports_none_rather_than_zero_for_a_reserve_it_has_never_seen` each observe
//! the *fold* through the store, and a [`Held`] has already lost what they
//! test: it carries a canonical `Address` rather than the checksummed one the
//! projection lower-cases, no `caller` for the projection to ignore, a share
//! balance rather than the deltas that summed to it, and nothing that can say
//! what a `LEFT JOIN` miss fills a column with. They stay with the adapter.

use alloy_primitives::{Address, U256};

use super::fixtures::{ALICE, BOB, RAY, SECOND_SPOKE, SPOKE, USDC, ask, reserve_ids};
use super::{PositionKey, PositionQuery, PositionStore};

/// One position a wallet holds, before anything values it.
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
/// The supply side is 1e6 shares against 1e6 of underlying and the rate is 5%
/// a year, so an amount is its share count at the checkpoint and 5% more a year
/// later — the arithmetic is `valuation`'s to prove, not this module's.
pub(crate) struct Listed {
    pub spoke: Address,
    pub reserve_id: &'static str,
    pub asset_id: &'static str,
    pub underlying: Address,
    pub decimals: u8,
    pub checkpoint_at: u64,
}

/// What an implementation supplies so the cases below have something to read.
pub(crate) trait Fixture {
    type Store: PositionStore;

    /// A store holding nothing, of this case's own.
    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;

    async fn given_positions(&self, held: &[Held]);
    async fn given_reserve(&self, listed: &Listed);
}

/// Deliberately out of numeric order as text: 13 sorts before 3.
const RESERVES: [&str; 5] = ["3", "7", "13", "21", "34"];

/// The block every fixture checkpoints at, and the instant it lands on.
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

// --- which rows come back ---------------------------------------------------

pub(crate) async fn returns_one_wallet_on_one_spoke_and_nobody_else<F: Fixture>() {
    let fixture = F::fresh("one_wallet").await;
    fixture
        .given_positions(&[
            Held::supplying("7", "500"),
            Held {
                supplied_shares: "900",
                ..Held::by(BOB, "7")
            },
        ])
        .await;

    // `user` is required rather than an optional filter: with `chain_id` it is
    // the leading pair of the sorting key, so a page is a seek into contiguous
    // rows rather than a scan.
    let page = fixture.store().list(&ask()).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].user, ALICE);
    assert_eq!(page.items[0].supplied_shares.to_string(), "500");
}

pub(crate) async fn finds_nothing_on_a_spoke_the_wallet_has_never_touched<F: Fixture>() {
    let fixture = F::fresh("untouched_spoke").await;
    fixture
        .given_positions(&[Held::supplying("7", "500")])
        .await;

    let elsewhere = PositionQuery {
        spoke: Some(Address::repeat_byte(0x11)),
        ..ask()
    };
    assert!(
        fixture
            .store()
            .list(&elsewhere)
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

pub(crate) async fn keeps_a_debt_only_position_with_no_supply_behind_it<F: Fixture>() {
    let fixture = F::fresh("debt_only").await;
    fixture
        .given_positions(&[Held {
            drawn_shares: "400",
            ..Held::by(ALICE, "13")
        }])
        .await;

    let page = fixture.store().list(&ask()).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].reserve_id, U256::from(13));
    assert_eq!(page.items[0].supplied_shares.to_string(), "0");
    assert_eq!(page.items[0].drawn_shares.to_string(), "400");
}

// --- across Spokes ----------------------------------------------------------

async fn both_spokes<F: Fixture>(case: &str) -> F {
    let fixture = F::fresh(case).await;
    fixture
        .given_positions(&[
            Held::supplying("7", "500"),
            Held {
                spoke: SECOND_SPOKE,
                supplied_shares: "900",
                ..Held::by(ALICE, "7")
            },
        ])
        .await;
    fixture
}

pub(crate) async fn lists_every_spoke_when_none_is_named_each_row_saying_which<F: Fixture>() {
    let fixture = both_spokes::<F>("every_spoke").await;

    // The same `reserveId` on two Spokes is two positions, not one — reserve
    // ids are Spoke-scoped, so nothing here may be merged on them. Every row
    // carries its own Spoke precisely so a caller can group rather than guess.
    let page = fixture
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

pub(crate) async fn narrows_to_one_when_it_is_named<F: Fixture>() {
    let fixture = both_spokes::<F>("one_spoke").await;

    let page = fixture
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

pub(crate) async fn walks_a_page_boundary_that_falls_between_two_spokes<F: Fixture>() {
    let fixture = both_spokes::<F>("spoke_boundary").await;

    // The half of the resume point that only exists once `spoke` is unpinned. A
    // `reserve_id`-only key would resume at "> 7" and lose the second Spoke's
    // reserve 7 entirely, because it sorts after the first Spoke's.
    let first = fixture
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

    let second = fixture
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

// --- order, and where a page stops ------------------------------------------

async fn five_reserves<F: Fixture>(case: &str) -> F {
    let fixture = F::fresh(case).await;
    let held: Vec<_> = RESERVES
        .iter()
        .map(|reserve_id| Held::supplying(reserve_id, "500"))
        .collect();
    fixture.given_positions(&held).await;
    fixture
}

pub(crate) async fn walks_every_position_exactly_once_in_numeric_order<F: Fixture>() {
    let fixture = five_reserves::<F>("walk").await;

    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = fixture
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

    // Keyset, so the boundary is a key rather than a row count: nothing is
    // returned twice or skipped even though the indexer is free to write
    // between pages. Ordered as UInt256, so 13 precedes 21 rather than 3.
    assert_eq!(seen, RESERVES);
}

pub(crate) async fn resumes_after_the_row_the_key_names_not_before_it<F: Fixture>() {
    let fixture = five_reserves::<F>("resume").await;

    let first = fixture
        .store()
        .list(&PositionQuery { limit: 2, ..ask() })
        .await
        .unwrap();
    let second = fixture
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

pub(crate) async fn reports_no_next_key_when_the_page_is_not_full<F: Fixture>() {
    let fixture = five_reserves::<F>("last_page").await;

    let full = fixture
        .store()
        .list(&PositionQuery { limit: 5, ..ask() })
        .await
        .unwrap();
    assert_eq!(full.next, None);

    let short = fixture
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

// --- the instant ------------------------------------------------------------

pub(crate) async fn values_every_position_on_a_page_at_one_instant<F: Fixture>() {
    let fixture = F::fresh("one_instant").await;
    fixture.given_reserve(&listed("7", "7")).await;
    fixture.given_reserve(&listed("13", "13")).await;
    fixture
        .given_positions(&[Held::supplying("7", "1000"), Held::supplying("13", "500")])
        .await;

    let page = fixture
        .store()
        .list(&PositionQuery {
            as_of: Some(CHECKPOINT_AT + YEAR),
            ..ask()
        })
        .await
        .unwrap();

    // Two positions, so the set below is evidence rather than a tautology: one
    // instant for the whole page, reported back, because an amount without the
    // moment it was computed at is not reproducible (§12.6).
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.valued_at, CHECKPOINT_AT + YEAR);
    let indexes: std::collections::BTreeSet<_> = page
        .items
        .iter()
        .map(|item| item.value.as_ref().map(|value| value.drawn_index))
        .collect();
    assert_eq!(indexes.len(), 1);
}

pub(crate) async fn defaults_to_now_when_no_instant_is_named<F: Fixture>() {
    let fixture = F::fresh("now").await;
    fixture.given_reserve(&listed("7", "7")).await;
    fixture
        .given_positions(&[Held::supplying("7", "1000")])
        .await;

    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let page = fixture.store().list(&ask()).await.unwrap();

    // Which is what the chain does — getUserDebt at `latest` extrapolates to
    // the head block rather than to the last event.
    assert!(page.valued_at >= before);
    assert_ne!(
        page.items[0]
            .value
            .as_ref()
            .unwrap()
            .drawn_index
            .to_string(),
        RAY
    );
}

/// Every case above, as one `#[tokio::test]` each against one implementation.
///
/// One test per case rather than one per suite, so a failure names the case and
/// `cargo` runs them concurrently. A case left out of the list below is a build
/// error rather than a silent gap: nothing else calls these, so it trips
/// `dead_code`, which the workspace denies.
macro_rules! position_store_contract {
    ($fixture:ty) => {
        $crate::store::contract::position_store_contract!(@cases $fixture:
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
            defaults_to_now_when_no_instant_is_named
        );
    };
    (@cases $fixture:ty: $($case:ident)*) => {
        $(
            #[tokio::test]
            async fn $case() {
                $crate::store::contract::$case::<$fixture>().await;
            }
        )*
    };
}

pub(crate) use position_store_contract;
