//! One page, assembled.
//!
//! Thin, because the fold is materialized views: nothing to compute. What it
//! owns is what the store does not — the listing a cursor belongs to, that a
//! chain with no cursor row is a missing resource rather than an empty list, and
//! the mapping onto a wire contract that moves at a different rate.
//!
//! **It is where the two enrichments are merged in**, keyed by chain rather than
//! by page and joined here rather than in SQL, which is what keeps the ClickHouse
//! adapter unaware either exists.

use std::collections::HashMap;

use aave_positions::store::{Position, PositionAsset, PositionQuery};
use aave_positions::valuation::{Valuation, to_value};
use alloy_primitives::{Address, I256};
use prices::{ReserveKey, ReservePrice};
use token_metadata::TokenLabel;

use super::cursor::Scope;
use super::params::Listing;
use super::scale::{self, ORACLE_DECIMALS, RAY_DECIMALS, VALUE_DECIMALS};
use super::{Asset, Item, Page, Pricing, Progress, Value, instant, instant_at};
use crate::app::App;
use crate::errors::{self, BoxedAppError};

/// Labels, keyed as the store hands them back.
type Labels = HashMap<Address, TokenLabel>;

/// Prices, keyed as the store hands them back.
type Prices = HashMap<ReserveKey, ReservePrice>;

/// The USD half of a position, or nothing when there is no price behind it.
struct Usd {
    price: String,
    supplied_amount: String,
    total_debt: String,
}

/// # Errors
///
/// A 404 when this deployment has never indexed the chain, a 400 for a cursor
/// that does not verify against this listing, and a 500 for anything the stores
/// or the arithmetic refuse.
pub(crate) async fn build(app: &App, listing: &Listing) -> Result<Page, BoxedAppError> {
    // Read first, and fail here rather than after a query that would answer "no
    // positions" for a chain this deployment does not follow.
    let Some(sync) = app.sync.get(listing.chain_id).await? else {
        return Err(errors::not_found(format!(
            "chain {} has not been indexed by this deployment",
            listing.chain_id
        )));
    };

    let scope = Scope {
        chain_id: listing.chain_id,
        user: listing.user,
        spoke: listing.spoke,
    };

    let query = PositionQuery {
        chain_id: listing.chain_id,
        user: listing.user,
        spoke: listing.spoke,
        limit: listing.limit,
        after: listing
            .cursor
            .as_deref()
            .map(|cursor| app.cursors.decode(cursor, &scope))
            .transpose()?,
        as_of: listing.as_of,
    };

    // Side by side: neither enrichment depends on which positions come back.
    // Measured on the service this replaces at 0.27 ms against a 28 ms page.
    let (page, labels, prices) = tokio::join!(
        app.positions.list(&query),
        app.tokens.labels(listing.chain_id),
        async {
            if listing.as_of.is_some() {
                // **An explicit `as_of` is served without prices at all**, and
                // skipped rather than discarded. Amounts extrapolate to that
                // instant; the stored price is whatever the oracle last said,
                // which is now. Pricing one against the other is a number that
                // never existed, and a price series is not what this stores.
                return Ok(Prices::new());
            }
            app.prices.latest(listing.chain_id).await
        }
    );
    let (page, labels, prices) = (page?, labels?, prices?);

    let items = page
        .items
        .iter()
        .map(|position| item(position, &labels, &prices))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Page {
        sync: Progress {
            last_block: sync.last_block,
            last_block_hash: sync.last_hash.to_string(),
            updated_at: instant(sync.updated_at)?,
            age_seconds: sync.age_seconds,
            stale: sync.age_seconds > app.staleness.sync,
        },
        valued_at: instant_at(page.valued_at)?,
        pricing: pricing(&page.items, &prices, app.staleness.price)?,
        items,
        next_cursor: page.next.map(|key| app.cursors.encode(&scope, &key)),
    })
}

/// The page's price clock, taken from the **oldest** price behind it.
///
/// Not the newest and not an average: what a caller needs is how far to trust
/// the worst number in front of them. It only diverges when the oracle refused a
/// reserve and its last good price was left to age.
///
/// Ties keep the earlier position, which is deterministic because the page is
/// ordered — `age_seconds` is floored, so two prices can tie and still differ in
/// `updated_at`.
fn pricing(
    positions: &[Position],
    prices: &Prices,
    stale_after: u64,
) -> Result<Option<Pricing>, BoxedAppError> {
    let oldest = positions
        .iter()
        .filter_map(|position| price_for(position, prices))
        .reduce(|worst, price| {
            if price.age_seconds > worst.age_seconds {
                price
            } else {
                worst
            }
        });

    oldest
        .map(|price| {
            Ok(Pricing {
                updated_at: instant(price.priced_at)?,
                age_seconds: price.age_seconds,
                stale: price.age_seconds > stale_after,
            })
        })
        .transpose()
}

/// The price this position would be valued with, if there is one.
///
/// Gated on `value` as well as `asset`: with nothing to price, a price is not
/// "used" and must not drag the page's clock backwards.
fn price_for<'a>(position: &Position, prices: &'a Prices) -> Option<&'a ReservePrice> {
    position.asset.as_ref()?;
    position.value.as_ref()?;

    prices.get(&ReserveKey {
        spoke: position.spoke,
        reserve_id: position.reserve_id,
    })
}

fn item(position: &Position, labels: &Labels, prices: &Prices) -> Result<Item, BoxedAppError> {
    // **The asset carries the scale.** An unscaled integer in a field the
    // contract calls decimal is wrong by up to eighteen orders of magnitude.
    let decimals = position.asset.as_ref().map(|asset| asset.decimals);
    let scaled = |amount: I256| decimals.map(|decimals| scale::signed(amount, decimals));

    let usd = position
        .asset
        .as_ref()
        .zip(position.value.as_ref())
        .zip(price_for(position, prices))
        .map(|((asset, value), price)| usd(asset, value, price))
        .transpose()?;

    Ok(Item {
        chain_id: position.chain_id,
        user: format!("{:#x}", position.user),
        spoke: format!("{:#x}", position.spoke),
        reserve_id: position.reserve_id.to_string(),
        supplied_shares: scaled(position.supplied_shares),
        drawn_shares: scaled(position.drawn_shares),
        premium_shares: scaled(position.premium_shares),
        // A ray is a ratio, so its scale is the protocol's fixed 27 and not the
        // asset's — which is why this one survives an unresolved reserve.
        premium_offset_ray: scale::signed(position.premium_offset_ray, RAY_DECIMALS),
        net_supplied_amount: scaled(position.net_supplied_amount),
        net_borrowed_amount: scaled(position.net_borrowed_amount),
        using_as_collateral: position.using_as_collateral,
        events: position.events,
        asset: position.asset.as_ref().map(|asset| {
            // Absent means enrichment has not reached the token; present with
            // a null symbol means it was asked and has none. Both serve null —
            // the store keeps them apart so the sweep knows what to do.
            let label = labels.get(&asset.underlying);

            Asset {
                asset_id: asset.asset_id.to_string(),
                hub: format!("{:#x}", asset.hub),
                underlying: format!("{:#x}", asset.underlying),
                decimals: asset.decimals,
                symbol: label.and_then(|label| label.symbol.clone()),
                name: label.and_then(|label| label.name.clone()),
            }
        }),
        value: position
            .value
            .as_ref()
            .zip(decimals)
            .map(|(value, decimals)| Value {
                supplied_amount: scale::unsigned(value.supplied_amount, decimals),
                drawn_debt: scale::unsigned(value.drawn_debt, decimals),
                premium_debt: scale::unsigned(value.premium_debt, decimals),
                total_debt: scale::unsigned(value.total_debt, decimals),
                drawn_index: scale::unsigned(value.drawn_index, RAY_DECIMALS),
                price_usd: usd.as_ref().map(|usd| usd.price.clone()),
                supplied_amount_usd: usd.as_ref().map(|usd| usd.supplied_amount.clone()),
                total_debt_usd: usd.as_ref().map(|usd| usd.total_debt.clone()),
            }),
    })
}

/// One position's USD half.
///
/// Computed in the protocol's unit and divided only on the way out: scaling the
/// inputs first rounds twice, and §7.1's reconciliation is exact or nothing.
///
/// `decimals` is the **Hub's**, from `AddAsset`, never the token's own. Where
/// they disagree, the Hub's is what the position is worth to Aave.
fn usd(
    asset: &PositionAsset,
    value: &Valuation,
    price: &ReservePrice,
) -> Result<Usd, BoxedAppError> {
    Ok(Usd {
        price: scale::unsigned(price.price, ORACLE_DECIMALS),
        supplied_amount: scale::unsigned(
            to_value(value.supplied_amount, asset.decimals, price.price)?,
            VALUE_DECIMALS,
        ),
        // Rounded up into token units, as the Spoke rounds a repayment: right
        // to display, wrong for a health factor, which divides an unrounded
        // ray-scaled debt. The two are meant to differ in the last digits.
        total_debt: scale::unsigned(
            to_value(value.total_debt, asset.decimals, price.price)?,
            VALUE_DECIMALS,
        ),
    })
}
