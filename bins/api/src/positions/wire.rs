//! What a page looks like on the wire, and how a folded position becomes one.
//!
//! The types and the conversion together, because the conversion has no other
//! subject: `item`, `pricing` and `usd` are constructors for the shapes below,
//! and splitting them off would name a layer rather than a thing.
//!
//! Four rules govern all six types. **No `rename_all` anywhere** — every
//! multi-word key is snake_case where the service this replaces writes
//! camelCase. **They mirror the domain rather than reuse it**, which is also why
//! the wire item is [`Item`]: `store::Position` may gain a field, the contract
//! may not. **Every number is a decimal string**, already scaled by [`scale`].
//! **Null means unknown, never zero** — §7.4's oracle reverts rather than answer
//! one, so a zero would be indistinguishable from a real one. Field order is
//! declaration order, and it is the order the service beside this one emits.

use std::collections::HashMap;

use aave_positions::store::{Position, PositionAsset};
use aave_positions::valuation::{Valuation, to_value};
use alloy_primitives::{Address, I256};
use prices::{ReserveKey, ReservePrice};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use token_metadata::TokenLabel;

use super::scale::{self, ORACLE_DECIMALS, RAY_DECIMALS, VALUE_DECIMALS};
use crate::errors::BoxedAppError;

/// One wallet's positions, valued at one instant.
#[derive(Debug, Serialize)]
pub(crate) struct Page {
    /// How current this payload is. Present on every page, because amounts are
    /// per-block quantities and a page whose sync differs from the previous one
    /// is the honest signal that the indexer advanced mid-walk.
    pub(crate) sync: Progress,

    /// When every amount on this page was computed — one instant for the whole
    /// page, so two positions in it cannot disagree about the time. Defaults to
    /// now, which is the same choice the chain makes. Distinct from
    /// `sync.last_block`: the shares are as far as the indexer has folded, the
    /// amounts are those shares valued at this instant.
    ///
    /// RFC 3339, matching `sync.updated_at` and `pricing.updated_at`. The
    /// `as_of` **query parameter** that sets it is Unix seconds, so
    /// round-tripping this value means converting it.
    pub(crate) valued_at: String,

    /// How current the prices behind this page are — **a third clock**, beside
    /// `sync` and `valued_at`, because prices have their own source and their
    /// own cadence. Null when nothing here is priced, and null whenever `as_of`
    /// is set: amounts are extrapolated to that instant and prices are not, so a
    /// value mixing the two would be a number that never existed.
    pub(crate) pricing: Option<Pricing>,

    pub(crate) items: Vec<Item>,

    /// Hand back verbatim as `cursor` to fetch the next page. `null` is the last
    /// page. Opaque and signed: it is only valid for the listing that issued it,
    /// so changing the wallet, chain or Spoke filter while reusing it is refused
    /// rather than silently resuming somewhere else.
    pub(crate) next_cursor: Option<String>,
}

/// How far the indexer has got on this chain.
#[derive(Debug, Serialize)]
pub(crate) struct Progress {
    /// The last block the indexer processed on this chain.
    pub(crate) last_block: u64,

    /// The hash the indexer saw at that height.
    pub(crate) last_block_hash: String,

    /// When the indexer last advanced, by the database clock that recorded it.
    pub(crate) updated_at: String,

    /// Seconds since the indexer last advanced. Measured on the server that
    /// wrote the timestamp, so it is not affected by clock skew between hosts.
    pub(crate) age_seconds: u64,

    /// Whether `age_seconds` has passed this deployment's freshness threshold. A
    /// stale response is still served — the numbers were true as of
    /// `last_block` — but they are behind the chain.
    pub(crate) stale: bool,
}

/// How current the prices behind this page are.
#[derive(Debug, Serialize)]
pub(crate) struct Pricing {
    /// When the oldest price behind any number on this page was read.
    updated_at: String,

    /// Seconds since then, measured by the database clock rather than this
    /// process's, so clock skew between the two cannot be reported as staleness.
    age_seconds: u64,

    /// Whether that exceeds this deployment's threshold. It measures how long
    /// since **we** last read the oracle, never how long since a feed last
    /// moved — an hour without a Chainlink update is normal behaviour, and a
    /// threshold set from feed cadence would flag healthy feeds forever.
    stale: bool,
}

/// One user's stake in one reserve on one Spoke.
#[derive(Debug, Serialize)]
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
    supplied_shares: Option<String>,

    /// Borrowed balance, in shares. Null on the same terms as `supplied_shares`.
    drawn_shares: Option<String>,

    /// Accrued risk premium, in shares. Null on the same terms.
    premium_shares: Option<String>,

    /// Premium offset, as a ray ratio. **Never null**, unlike the share fields
    /// beside it: a ray's scale is the protocol's fixed 27 rather than the
    /// asset's, so it can be rendered whether or not the registry has resolved
    /// the reserve.
    premium_offset_ray: String,

    /// Net principal supplied, in asset units. A *flow*, not a balance: it sums
    /// what the events carried, and between events the interest index accrues
    /// while emitting nothing. Null on the same terms as `supplied_shares`.
    net_supplied_amount: Option<String>,

    /// Net principal borrowed, in asset units. Also a flow.
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
    asset: Option<Asset>,

    /// The shares above, converted to whole tokens at `valued_at`. Null when
    /// `asset` is, and also when the Hub has listed the asset but not yet
    /// checkpointed its index — a zero there could not be told apart from a real
    /// zero balance.
    value: Option<Value>,
}

/// What a reserve refers to, once the registry and the Hub have both been read.
#[derive(Debug, Serialize)]
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
    symbol: Option<String>,

    /// The token's own `name()`. Null on the same terms as `symbol`.
    name: Option<String>,
}

/// What one position is worth at `valued_at`.
#[derive(Debug, Serialize)]
struct Value {
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
    price_usd: Option<String>,

    /// What `supplied_amount` is worth, in dollars. Every digit the protocol
    /// computed is kept — §7.1's unit puts `1e26` at one dollar, and this is
    /// that number divided rather than rounded, so it still reconciles against
    /// `getUserAccountData` exactly. Null on the same terms as `price_usd`.
    supplied_amount_usd: Option<String>,

    /// What `total_debt` is worth, in dollars. Note this prices the **rounded**
    /// token amount, which is what is owed and what a caller should display. The
    /// health factor is computed from an unrounded ray-scaled debt instead, so
    /// the two will differ in the last digits by design.
    total_debt_usd: Option<String>,
}

/// The four widths a subsecond may take, and nothing between them: the SI
/// buckets, which is `chrono`'s `SecondsFormat::AutoSi`. Measured over 6,126
/// timestamps from crates.io's API — 2,182 with no fraction, one with three
/// digits, 3,943 with six, none with any other width.
const WHOLE: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
const MILLIS: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
const MICROS: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");
const NANOS: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]Z");

/// The wire's spelling of an instant: RFC 3339, in UTC, at an SI width.
///
/// **Neither of the two obvious formats.** `toISOString` emits three digits
/// always and truncates below them, throwing away the microseconds Postgres
/// stored. `time`'s `Rfc3339` trims *every* trailing zero, so `…00.5+00` goes
/// out as `…00.5Z` — a width `datetime.fromisoformat` refuses before Python
/// 3.11, and one nothing else in Rust emits.
///
/// **It does not fix sorting**, which it reads as though it would: `.` is below
/// `Z`, so `…00.500Z` sorts before `…00Z` at any width. These are instants, not
/// sort keys.
///
/// UTC is forced rather than assumed — a non-zero offset would print `+02:00`
/// where every reader of this field expects `Z`.
///
/// # Errors
///
/// [`time::error::Format`], which needs an unrepresentable year to produce.
pub(crate) fn instant(at: OffsetDateTime) -> Result<String, time::error::Format> {
    let at = at.to_offset(time::UtcOffset::UTC);

    at.format(match at.nanosecond() {
        0 => WHOLE,
        nanos if nanos % 1_000_000 == 0 => MILLIS,
        nanos if nanos % 1_000 == 0 => MICROS,
        _ => NANOS,
    })
}

/// The same, from the Unix seconds the store values a page at.
///
/// # Errors
///
/// [`time::Error`] when the seconds are outside the range a date can hold, which
/// `as_of`'s bounds already exclude.
pub(crate) fn instant_at(seconds: u64) -> Result<String, time::Error> {
    // The same fault an out-of-range timestamp produces, a century early.
    let seconds = i64::try_from(seconds).map_err(|_| time::error::ConversionRange)?;

    Ok(instant(OffsetDateTime::from_unix_timestamp(seconds)?)?)
}

type Labels = HashMap<Address, TokenLabel>;

/// Prices, keyed as the store hands them back.
pub(crate) type Prices = HashMap<ReserveKey, ReservePrice>;

/// The USD half of a position, or nothing when there is no price behind it.
struct Usd {
    price: String,
    supplied_amount: String,
    total_debt: String,
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
pub(crate) fn pricing(
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

pub(crate) fn item(
    position: &Position,
    labels: &Labels,
    prices: &Prices,
) -> Result<Item, BoxedAppError> {
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

#[cfg(test)]
mod tests {
    //! The one spelling of a timestamp, and the two shapes it is not.

    use time::OffsetDateTime;

    use super::{instant, instant_at};

    #[test]
    fn writes_a_whole_second_without_a_fractional_part() {
        // Where `toISOString` writes `.000Z`. The difference is the declared
        // one, and it is pinned here so it cannot drift back by accident.
        assert_eq!(
            instant_at(1_788_796_630).ok().as_deref(),
            Some("2026-09-07T15:57:10Z")
        );
    }

    #[test]
    fn pads_a_fraction_up_to_the_next_si_width() {
        // The whole point of the buckets. `time`'s `Rfc3339` writes `.5Z` here,
        // which a strict parser refuses and which sorts after a whole second in
        // the same second.
        let half = OffsetDateTime::from_unix_timestamp_nanos(1_785_000_000_500_000_000)
            .expect("a representable instant");

        assert_eq!(
            instant(half).ok().as_deref(),
            Some("2026-07-25T17:20:00.500Z")
        );
    }

    #[test]
    fn takes_each_si_width_and_nothing_between_them() {
        let at = |nanos: i128| {
            instant(
                OffsetDateTime::from_unix_timestamp_nanos(1_785_000_000_000_000_000 + nanos)
                    .expect("a representable instant"),
            )
            .ok()
            .unwrap_or_default()
        };

        for (nanos, expected) in [
            (0, "2026-07-25T17:20:00Z"),
            (1_000_000, "2026-07-25T17:20:00.001Z"),
            (221_000_000, "2026-07-25T17:20:00.221Z"),
            (221_456_000, "2026-07-25T17:20:00.221456Z"),
            (221_456_789, "2026-07-25T17:20:00.221456789Z"),
        ] {
            assert_eq!(at(nanos), expected, "{nanos} nanoseconds");
        }
    }

    #[test]
    fn keeps_the_microseconds_postgres_stored() {
        // `toISOString` truncates these to three digits. Postgres holds six, and
        // this is the field a caller compares against a row.
        let at = OffsetDateTime::from_unix_timestamp_nanos(1_785_000_000_221_456_000)
            .expect("a representable instant");

        assert_eq!(
            instant(at).ok().as_deref(),
            Some("2026-07-25T17:20:00.221456Z")
        );
    }

    #[test]
    fn writes_zulu_rather_than_a_zero_offset() {
        let at = OffsetDateTime::from_unix_timestamp(1_785_000_000)
            .expect("a representable instant")
            .to_offset(time::UtcOffset::from_hms(2, 0, 0).expect("a real offset"));

        assert_eq!(instant(at).ok().as_deref(), Some("2026-07-25T17:20:00Z"));
    }

    #[test]
    fn refuses_an_instant_no_date_can_hold_rather_than_wrapping() {
        assert!(instant_at(u64::MAX).is_err());
    }
}
