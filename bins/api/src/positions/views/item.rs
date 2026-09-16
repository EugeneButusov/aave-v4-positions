//! One position on the wire, and what a folded one becomes.

use aave_positions::scale::{self, ORACLE_DECIMALS, RAY_DECIMALS, VALUE_DECIMALS};
use aave_positions::store::{Position, PositionAsset};
use aave_positions::valuation::{Valuation, to_value};
use alloy_primitives::I256;
use prices::ReservePrice;
use serde::Serialize;
use utoipa::ToSchema;

use super::{Error, Labels, Prices, price_for};

/// One user's stake in one reserve on one Spoke.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Item {
    chain_id: u32,

    /// The position owner, lower-cased. Never the caller that routed the action
    /// — position managers act on behalf of users, and crediting them would
    /// attribute large parts of the book to a handful of router addresses.
    user: String,

    /// The Spoke this position lives on. Always present, including when the
    /// request did not filter on one. Two Spokes are two isolated margin
    /// accounts with their own collateral factors, oracle and health factor, so
    /// positions on them may be listed together but must never be summed.
    spoke: String,

    /// The Spoke's own index for the reserve, and the only asset identity a
    /// position has today. Not a protocol-wide asset id and not a token address
    /// — no Spoke event carries one. The same id on two Spokes means two
    /// different things.
    reserve_id: String,

    /// Supplied balance, in shares, scaled by the asset's decimals. Not an asset
    /// amount. **Null exactly when `asset` is**: the scale lives on the asset,
    /// so without it there is no honest way to render this, and an unscaled
    /// integer in a field documented as decimal would be wrong by orders of
    /// magnitude.
    #[schema(required = true)]
    supplied_shares: Option<String>,

    /// Borrowed balance, in shares. Null on the same terms as `supplied_shares`.
    #[schema(required = true)]
    drawn_shares: Option<String>,

    /// Accrued risk premium, in shares. Null on the same terms.
    #[schema(required = true)]
    premium_shares: Option<String>,

    /// Premium offset, as a ray ratio. **Never null**, unlike the share fields
    /// beside it: a ray's scale is the protocol's fixed 27 rather than the
    /// asset's, so it can be rendered whether or not the registry has resolved
    /// the reserve.
    premium_offset_ray: String,

    /// Net principal supplied, in asset units. A *flow*, not a balance: it sums
    /// what the events carried, and between events the interest index accrues
    /// while emitting nothing. Null on the same terms as `supplied_shares`.
    #[schema(required = true)]
    net_supplied_amount: Option<String>,

    /// Net principal borrowed, in asset units. Also a flow.
    #[schema(required = true)]
    net_borrowed_amount: Option<String>,

    /// The user's own collateral flag, and only that. It is not the `collateral`
    /// position type, which additionally requires a non-zero collateral factor
    /// under the version of the reserve config the user is pinned to — config
    /// events this build does not ingest. Five of the Main Spoke's fourteen
    /// reserves sit at a zero factor, so this flag alone overstates collateral
    /// for those.
    using_as_collateral: bool,

    /// Ledger rows folded into this position. Never zero for a returned one.
    events: i32,

    /// What `reserve_id` actually refers to, once the registry and the Hub have
    /// both been read. Null with `value` when the join has nothing to offer.
    #[schema(required = true)]
    asset: Option<Asset>,

    /// The shares above, converted to whole tokens at `valued_at`. Null when
    /// `asset` is, and also when the Hub has listed the asset but not yet
    /// checkpointed its index — a zero there could not be told apart from a real
    /// zero balance.
    #[schema(required = true)]
    value: Option<Worth>,
}

impl Item {
    /// # Errors
    ///
    /// [`Error`], when this position's dollar value cannot be computed.
    pub(crate) fn new(
        position: &Position,
        labels: &Labels,
        prices: &Prices,
    ) -> Result<Self, Error> {
        // **The asset carries the scale.** An unscaled integer in a field the
        // contract calls decimal is wrong by up to eighteen orders of magnitude.
        let decimals = position.asset.as_ref().map(|asset| asset.decimals);
        let scaled = |amount: I256| decimals.map(|decimals| scale::signed(amount, decimals));

        let usd = position
            .asset
            .as_ref()
            .zip(position.value.as_ref())
            .zip(price_for(position, prices))
            .map(|((asset, value), price)| Usd::new(asset, value, price))
            .transpose()?;

        Ok(Self {
            chain_id: position.chain_id,
            user: format!("{:#x}", position.user),
            spoke: format!("{:#x}", position.spoke),
            reserve_id: position.reserve_id.to_string(),
            supplied_shares: scaled(position.supplied_shares),
            drawn_shares: scaled(position.drawn_shares),
            premium_shares: scaled(position.premium_shares),
            // A ray is a ratio, so its scale is the protocol's fixed 27 and not
            // the asset's — which is why this one survives an unresolved reserve.
            premium_offset_ray: scale::signed(position.premium_offset_ray, RAY_DECIMALS),
            net_supplied_amount: scaled(position.net_supplied_amount),
            net_borrowed_amount: scaled(position.net_borrowed_amount),
            using_as_collateral: position.using_as_collateral,
            events: position.events,
            asset: position
                .asset
                .as_ref()
                .map(|asset| Asset::new(asset, labels)),
            value: position
                .value
                .as_ref()
                .zip(decimals)
                .map(|(value, decimals)| Worth::new(value, decimals, usd.as_ref())),
        })
    }
}

/// What a reserve refers to, once the registry and the Hub have both been read.
#[derive(Debug, Serialize, ToSchema)]
struct Asset {
    /// The Hub's id for the asset, which is what makes it comparable across
    /// Spokes.
    asset_id: String,

    /// The Hub holding the liquidity.
    hub: String,

    /// The ERC-20 itself. It appears in no Spoke event — resolving it needs the
    /// reserve registry and the Hub's own asset listing.
    underlying: String,

    /// Token decimals, as the Hub listed them.
    decimals: u8,

    /// The token's own `symbol()`. **A label, not an identity** — nothing stops
    /// two tokens claiming the same one. `underlying` is the identity. Null when
    /// the token has no `symbol()`, which ERC-20 permits, and also in the window
    /// before enrichment has reached a newly listed asset.
    #[schema(required = true)]
    symbol: Option<String>,

    /// The token's own `name()`. Null on the same terms as `symbol`.
    #[schema(required = true)]
    name: Option<String>,
}

impl Asset {
    fn new(asset: &PositionAsset, labels: &Labels) -> Self {
        // Absent means enrichment has not reached the token; present with a
        // null symbol means it was asked and has none. Both serve null — the
        // store keeps them apart so the sweep knows what to do.
        let label = labels.get(&asset.underlying);

        Self {
            asset_id: asset.asset_id.to_string(),
            hub: format!("{:#x}", asset.hub),
            underlying: format!("{:#x}", asset.underlying),
            decimals: asset.decimals,
            symbol: label.and_then(|label| label.symbol.clone()),
            name: label.and_then(|label| label.name.clone()),
        }
    }
}

// Not `Value`: utoipa matches type names by their last segment, so a schema
// called that is published as `serde_json::Value`, losing every field below.
/// What one position is worth at `valued_at`.
#[derive(Debug, Serialize, ToSchema)]
struct Worth {
    /// Underlying redeemable for the supplied shares, in whole tokens, rounded
    /// down as the Hub does.
    supplied_amount: String,

    /// Principal debt, rounded up as the Spoke does.
    drawn_debt: String,

    /// Accrued risk premium, in whole tokens.
    premium_debt: String,

    /// `drawn_debt + premium_debt`.
    total_debt: String,

    /// The interest index this valuation used: the last checkpoint extrapolated
    /// to `valued_at`, as a ray ratio where `1` is no accrual. Published so a
    /// caller can reproduce the arithmetic rather than trust it.
    drawn_index: String,

    /// What Aave's own oracle prices one whole token at, in dollars. This is the
    /// protocol's view rather than the market's, which is the right one for a
    /// position: it is the number that drives liquidation. Null when the oracle
    /// has not been read for this reserve yet, or when `as_of` is set.
    #[schema(required = true)]
    price_usd: Option<String>,

    /// What `supplied_amount` is worth, in dollars. Every digit the protocol
    /// computed is kept — §7.1's unit puts `1e26` at one dollar, and this is
    /// that number divided rather than rounded, so it still reconciles against
    /// `getUserAccountData` exactly. Null on the same terms as `price_usd`.
    #[schema(required = true)]
    supplied_amount_usd: Option<String>,

    /// What `total_debt` is worth, in dollars. Note this prices the **rounded**
    /// token amount, which is what is owed and what a caller should display. The
    /// health factor is computed from an unrounded ray-scaled debt instead, so
    /// the two will differ in the last digits by design.
    #[schema(required = true)]
    total_debt_usd: Option<String>,
}

impl Worth {
    fn new(value: &Valuation, decimals: u8, usd: Option<&Usd>) -> Self {
        Self {
            supplied_amount: scale::unsigned(value.supplied_amount, decimals),
            drawn_debt: scale::unsigned(value.drawn_debt, decimals),
            premium_debt: scale::unsigned(value.premium_debt, decimals),
            total_debt: scale::unsigned(value.total_debt, decimals),
            drawn_index: scale::unsigned(value.drawn_index, RAY_DECIMALS),
            price_usd: usd.map(|usd| usd.price.clone()),
            supplied_amount_usd: usd.map(|usd| usd.supplied_amount.clone()),
            total_debt_usd: usd.map(|usd| usd.total_debt.clone()),
        }
    }
}

/// The USD half of a position, or nothing when there is no price behind it.
struct Usd {
    price: String,
    supplied_amount: String,
    total_debt: String,
}

impl Usd {
    /// Computed in the protocol's unit and divided only on the way out: scaling
    /// the inputs first rounds twice, and §7.1's reconciliation is exact or
    /// nothing.
    ///
    /// `decimals` is the **Hub's**, from `AddAsset`, never the token's own.
    /// Where they disagree, the Hub's is what the position is worth to Aave.
    fn new(asset: &PositionAsset, value: &Valuation, price: &ReservePrice) -> Result<Self, Error> {
        Ok(Self {
            price: scale::unsigned(price.price, ORACLE_DECIMALS),
            supplied_amount: scale::unsigned(
                to_value(value.supplied_amount, asset.decimals, price.price)?,
                VALUE_DECIMALS,
            ),
            // Rounded up into token units, as the Spoke rounds a repayment:
            // right to display, wrong for a health factor, which divides an
            // unrounded ray-scaled debt. The two are meant to differ in the
            // last digits.
            total_debt: scale::unsigned(
                to_value(value.total_debt, asset.decimals, price.price)?,
                VALUE_DECIMALS,
            ),
        })
    }
}
