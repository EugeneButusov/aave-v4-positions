//! The [`ReservePriceStore`] port as an executable specification.
//!
//! Every implementation runs it. What the port cannot supply is the rows
//! themselves — it is read-only until the refresher arrives — so an
//! implementation also supplies a [`Harness`] saying how state gets in front of
//! it.

use std::collections::HashMap;

use alloy_primitives::{Address, U256, address};

use crate::{ReserveKey, ReservePrice, ReservePriceStore};

pub(crate) const CHAIN_ID: u32 = 1;
pub(crate) const OTHER_CHAIN: u32 = 8453;

pub(crate) const SPOKE: Address = address!("94e7a5dcbe816e498b89ab752661904e2f56c485");
pub(crate) const SECOND_SPOKE: Address = address!("1111111111111111111111111111111111111111");

/// `$1.00`, at §7.4's eight decimals.
pub(crate) const ONE_DOLLAR: &str = "100000000";

/// One row of `reserve_prices`, as a case wants to describe it.
pub(crate) struct Quoted {
    pub chain_id: u32,
    pub spoke: Address,
    pub reserve_id: u64,
    /// Decimal, so a case can write one wider than a `f64` holds.
    pub price: &'static str,
    /// How far in the past the database should stamp `priced_at`.
    pub aged_seconds: u64,
}

impl Quoted {
    /// Every field, named once.
    fn row(
        chain_id: u32,
        spoke: Address,
        reserve_id: u64,
        price: &'static str,
        aged_seconds: u64,
    ) -> Self {
        Self {
            chain_id,
            spoke,
            reserve_id,
            price,
            aged_seconds,
        }
    }

    pub(crate) fn at(chain_id: u32, reserve_id: u64, price: &'static str) -> Self {
        Self::row(chain_id, SPOKE, reserve_id, price, 0)
    }

    pub(crate) fn on_spoke(
        chain_id: u32,
        spoke: Address,
        reserve_id: u64,
        price: &'static str,
    ) -> Self {
        Self::row(chain_id, spoke, reserve_id, price, 0)
    }

    pub(crate) fn aged(chain_id: u32, reserve_id: u64, aged_seconds: u64) -> Self {
        Self::row(chain_id, SPOKE, reserve_id, ONE_DOLLAR, aged_seconds)
    }
}

pub(crate) fn key(spoke: Address, reserve_id: u64) -> ReserveKey {
    ReserveKey {
        spoke,
        reserve_id: U256::from(reserve_id),
    }
}

pub(crate) trait Harness {
    type Store: ReservePriceStore;

    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;

    async fn given_prices(&self, rows: &[Quoted]);
}

async fn prices_of<H: Harness>(harness: &H) -> HashMap<ReserveKey, ReservePrice> {
    harness
        .store()
        .latest(CHAIN_ID)
        .await
        .expect("the dimension should read")
}

pub(crate) async fn reads_every_price_on_the_chain<H: Harness>() {
    let harness = H::fresh("reads_every_price_on_the_chain").await;
    harness
        .given_prices(&[
            Quoted::at(CHAIN_ID, 1, ONE_DOLLAR),
            Quoted::at(CHAIN_ID, 2, "300000000000"),
        ])
        .await;

    let prices = prices_of(&harness).await;

    assert_eq!(prices.len(), 2);
    assert_eq!(prices[&key(SPOKE, 1)].price, U256::from(100_000_000_u64));
    assert_eq!(
        prices[&key(SPOKE, 2)].price,
        U256::from(300_000_000_000_u64)
    );
}

pub(crate) async fn leaves_out_a_chain_it_was_not_asked_about<H: Harness>() {
    let harness = H::fresh("leaves_out_a_chain_it_was_not_asked_about").await;
    harness
        .given_prices(&[
            Quoted::at(CHAIN_ID, 1, ONE_DOLLAR),
            Quoted::at(OTHER_CHAIN, 2, ONE_DOLLAR),
        ])
        .await;

    let prices = prices_of(&harness).await;

    assert_eq!(prices.len(), 1, "{prices:?}");
    assert!(prices.contains_key(&key(SPOKE, 1)));
}

pub(crate) async fn keeps_two_spokes_pricing_the_same_reserve_id_apart<H: Harness>() {
    // §12.3: each Spoke is an isolated margin account with its own oracle, so
    // the same reserve id on two of them is two prices and they are allowed to
    // disagree. A key that was the reserve id alone would silently pick one.
    let harness = H::fresh("keeps_two_spokes_pricing_the_same_reserve_id_apart").await;
    harness
        .given_prices(&[
            Quoted::on_spoke(CHAIN_ID, SPOKE, 1, ONE_DOLLAR),
            Quoted::on_spoke(CHAIN_ID, SECOND_SPOKE, 1, "99000000"),
        ])
        .await;

    let prices = prices_of(&harness).await;

    assert_eq!(prices.len(), 2);
    assert_eq!(prices[&key(SPOKE, 1)].price, U256::from(100_000_000_u64));
    assert_eq!(
        prices[&key(SECOND_SPOKE, 1)].price,
        U256::from(99_000_000_u64)
    );
}

pub(crate) async fn omits_a_reserve_nobody_has_priced<H: Harness>() {
    // Absent rather than zero: the oracle reverts on a zero price (§7.4), so a
    // zero here could only ever be our own invention, and the read path serves
    // null for a miss.
    let harness = H::fresh("omits_a_reserve_nobody_has_priced").await;
    harness
        .given_prices(&[Quoted::at(CHAIN_ID, 1, ONE_DOLLAR)])
        .await;

    assert!(!prices_of(&harness).await.contains_key(&key(SPOKE, 999)));
}

pub(crate) async fn answers_a_checksummed_spoke_from_a_lower_cased_row<H: Harness>() {
    // What the TypeScript's `reserveKey` exists to get right by discipline.
    let harness = H::fresh("answers_a_checksummed_spoke_from_a_lower_cased_row").await;
    harness
        .given_prices(&[Quoted::at(CHAIN_ID, 1, ONE_DOLLAR)])
        .await;

    let checksummed: Address = "0x94E7a5DcBe816e498B89aB752661904e2F56c485"
        .parse()
        .expect("a checksummed address parses");

    assert!(prices_of(&harness).await.contains_key(&key(checksummed, 1)));
}

pub(crate) async fn reads_a_price_past_what_a_double_can_hold<H: Harness>() {
    // §7.5. `numeric(78,0)` holds it and a JSON number does not, which is why
    // the column comes back as text and is parsed here.
    const HUGE: &str = "123456789012345678901234567890";

    let harness = H::fresh("reads_a_price_past_what_a_double_can_hold").await;
    harness.given_prices(&[Quoted::at(CHAIN_ID, 1, HUGE)]).await;

    assert_eq!(
        prices_of(&harness).await[&key(SPOKE, 1)].price,
        U256::from_str_radix(HUGE, 10).expect("the literal is a uint256"),
    );
}

pub(crate) async fn ages_a_price_by_the_database_clock<H: Harness>() {
    // Computed by the server that wrote the timestamp, not by this process —
    // a reader subtracting its own clock reports skew as staleness.
    let harness = H::fresh("ages_a_price_by_the_database_clock").await;
    harness
        .given_prices(&[
            Quoted::aged(CHAIN_ID, 1, 90),
            Quoted::at(CHAIN_ID, 2, ONE_DOLLAR),
        ])
        .await;

    let prices = prices_of(&harness).await;

    assert!(
        (90..95).contains(&prices[&key(SPOKE, 1)].age_seconds),
        "{}",
        prices[&key(SPOKE, 1)].age_seconds
    );
    assert!(prices[&key(SPOKE, 2)].age_seconds < 5);
}
