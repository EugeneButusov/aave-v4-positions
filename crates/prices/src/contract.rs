//! The [`ReservePriceStore`] port as an executable specification.
//!
//! Every implementation runs it. What the port cannot supply is the rows
//! themselves — it is read-only until the refresher arrives — so an
//! implementation also supplies a [`Fixture`] saying how state gets in front of
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
    pub(crate) fn at(reserve_id: u64, price: &'static str) -> Self {
        Self {
            chain_id: CHAIN_ID,
            spoke: SPOKE,
            reserve_id,
            price,
            aged_seconds: 0,
        }
    }

    pub(crate) fn on_spoke(spoke: Address, reserve_id: u64, price: &'static str) -> Self {
        Self {
            spoke,
            ..Self::at(reserve_id, price)
        }
    }

    pub(crate) fn on_chain(chain_id: u32, reserve_id: u64) -> Self {
        Self {
            chain_id,
            ..Self::at(reserve_id, ONE_DOLLAR)
        }
    }

    pub(crate) fn aged(reserve_id: u64, seconds: u64) -> Self {
        Self {
            aged_seconds: seconds,
            ..Self::at(reserve_id, ONE_DOLLAR)
        }
    }
}

pub(crate) fn key(spoke: Address, reserve_id: u64) -> ReserveKey {
    ReserveKey {
        spoke,
        reserve_id: U256::from(reserve_id),
    }
}

pub(crate) trait Fixture {
    type Store: ReservePriceStore;

    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;

    async fn given_prices(&self, rows: &[Quoted]);
}

async fn prices_of<F: Fixture>(fixture: &F) -> HashMap<ReserveKey, ReservePrice> {
    fixture
        .store()
        .latest(CHAIN_ID)
        .await
        .expect("the dimension should read")
}

pub(crate) async fn reads_every_price_on_the_chain<F: Fixture>() {
    let fixture = F::fresh("reads_every_price_on_the_chain").await;
    fixture
        .given_prices(&[Quoted::at(1, ONE_DOLLAR), Quoted::at(2, "300000000000")])
        .await;

    let prices = prices_of(&fixture).await;

    assert_eq!(prices.len(), 2);
    assert_eq!(prices[&key(SPOKE, 1)].price, U256::from(100_000_000_u64));
    assert_eq!(
        prices[&key(SPOKE, 2)].price,
        U256::from(300_000_000_000_u64)
    );
}

pub(crate) async fn leaves_out_a_chain_it_was_not_asked_about<F: Fixture>() {
    let fixture = F::fresh("leaves_out_a_chain_it_was_not_asked_about").await;
    fixture
        .given_prices(&[Quoted::at(1, ONE_DOLLAR), Quoted::on_chain(OTHER_CHAIN, 2)])
        .await;

    let prices = prices_of(&fixture).await;

    assert_eq!(prices.len(), 1, "{prices:?}");
    assert!(prices.contains_key(&key(SPOKE, 1)));
}

pub(crate) async fn keeps_two_spokes_pricing_the_same_reserve_id_apart<F: Fixture>() {
    // §12.3: each Spoke is an isolated margin account with its own oracle, so
    // the same reserve id on two of them is two prices and they are allowed to
    // disagree. A key that was the reserve id alone would silently pick one.
    let fixture = F::fresh("keeps_two_spokes_pricing_the_same_reserve_id_apart").await;
    fixture
        .given_prices(&[
            Quoted::on_spoke(SPOKE, 1, ONE_DOLLAR),
            Quoted::on_spoke(SECOND_SPOKE, 1, "99000000"),
        ])
        .await;

    let prices = prices_of(&fixture).await;

    assert_eq!(prices.len(), 2);
    assert_eq!(prices[&key(SPOKE, 1)].price, U256::from(100_000_000_u64));
    assert_eq!(
        prices[&key(SECOND_SPOKE, 1)].price,
        U256::from(99_000_000_u64)
    );
}

pub(crate) async fn omits_a_reserve_nobody_has_priced<F: Fixture>() {
    // Absent rather than zero: the oracle reverts on a zero price (§7.4), so a
    // zero here could only ever be our own invention, and the read path serves
    // null for a miss.
    let fixture = F::fresh("omits_a_reserve_nobody_has_priced").await;
    fixture.given_prices(&[Quoted::at(1, ONE_DOLLAR)]).await;

    assert!(!prices_of(&fixture).await.contains_key(&key(SPOKE, 999)));
}

pub(crate) async fn answers_a_checksummed_spoke_from_a_lower_cased_row<F: Fixture>() {
    // What the TypeScript's `reserveKey` exists to get right by discipline.
    let fixture = F::fresh("answers_a_checksummed_spoke_from_a_lower_cased_row").await;
    fixture.given_prices(&[Quoted::at(1, ONE_DOLLAR)]).await;

    let checksummed: Address = "0x94E7a5DcBe816e498B89aB752661904e2F56c485"
        .parse()
        .expect("a checksummed address parses");

    assert!(prices_of(&fixture).await.contains_key(&key(checksummed, 1)));
}

pub(crate) async fn reads_a_price_past_what_a_double_can_hold<F: Fixture>() {
    // §7.5. `numeric(78,0)` holds it and a JSON number does not, which is why
    // the column comes back as text and is parsed here.
    const HUGE: &str = "123456789012345678901234567890";

    let fixture = F::fresh("reads_a_price_past_what_a_double_can_hold").await;
    fixture.given_prices(&[Quoted::at(1, HUGE)]).await;

    assert_eq!(
        prices_of(&fixture).await[&key(SPOKE, 1)].price,
        U256::from_str_radix(HUGE, 10).expect("the literal is a uint256"),
    );
}

pub(crate) async fn ages_a_price_by_the_database_clock<F: Fixture>() {
    // Computed by the server that wrote the timestamp, not by this process —
    // a reader subtracting its own clock reports skew as staleness.
    let fixture = F::fresh("ages_a_price_by_the_database_clock").await;
    fixture
        .given_prices(&[Quoted::aged(1, 90), Quoted::at(2, ONE_DOLLAR)])
        .await;

    let prices = prices_of(&fixture).await;

    assert!(
        (90..95).contains(&prices[&key(SPOKE, 1)].age_seconds),
        "{}",
        prices[&key(SPOKE, 1)].age_seconds
    );
    assert!(prices[&key(SPOKE, 2)].age_seconds < 5);
}
