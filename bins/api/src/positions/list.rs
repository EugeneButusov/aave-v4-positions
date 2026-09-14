//! `GET /{prefix}/v1/chains/{chain_id}/users/{user}/positions`.
//!
//! Thin, because the fold is materialized views: nothing to compute. What it
//! owns is what the store does not — the listing a cursor belongs to, that a
//! chain with no cursor row is a missing resource rather than an empty list, and
//! the mapping onto a wire contract that moves at a different rate.
//!
//! **It is where the two enrichments are merged in**, keyed by chain rather than
//! by page and joined here rather than in SQL, which is what keeps the ClickHouse
//! adapter unaware either exists.
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

use aave_positions::store::{Position, PositionAsset, PositionQuery};
use aave_positions::valuation::{Valuation, to_value};
use alloy_primitives::{Address, I256};
use axum::extract::{Path, RawQuery, State};
use axum::routing::get;
use axum::{Json, Router};
use prices::{ReserveKey, ReservePrice};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;
use token_metadata::TokenLabel;

use super::cursor::Scope;
use super::params::Listing;
use super::scale::{self, ORACLE_DECIMALS, RAY_DECIMALS, VALUE_DECIMALS};
use crate::app::AppState;
use crate::errors::{self, BoxedAppError};

/// The paths this module serves, relative to the version prefix `router` mounts
/// them under.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/chains/{chain_id}/users/{user}/positions", get(list))
}

/// **The query arrives raw rather than deserialized**: neither an unrecognised
/// key nor every fault at once survives a `Deserialize`.
/// # Errors
///
/// A 404 when this deployment has never indexed the chain, a 400 for a cursor
/// that does not verify against this listing, and a 500 for anything the stores
/// or the arithmetic refuse.
async fn list(
    State(app): State<AppState>,
    Path((chain_id, user)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Json<Page>, BoxedAppError> {
    let listing = Listing::parse(&chain_id, &user, query.as_deref().unwrap_or_default())?;

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
            // **The codec says what was wrong; this says what it costs.** It
            // knows nothing of statuses, so the demotion to a 400 happens here,
            // where the answer is already an HTTP one.
            .map(|cursor| {
                app.signer.decode(cursor, &scope).map_err(|invalid| {
                    errors::bad_request(format!("invalid page cursor: {invalid}"))
                })
            })
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

    Ok(Json(Page {
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
        next_cursor: page.next.map(|key| app.signer.encode(&scope, &key)),
    }))
}

/// One wallet's positions, valued at one instant.
#[derive(Debug, Serialize)]

struct Page {
    /// How current this payload is. Present on every page, because amounts are
    /// per-block quantities and a page whose sync differs from the previous one
    /// is the honest signal that the indexer advanced mid-walk.
    sync: Progress,

    /// When every amount on this page was computed — one instant for the whole
    /// page, so two positions in it cannot disagree about the time. Defaults to
    /// now, which is the same choice the chain makes. Distinct from
    /// `sync.last_block`: the shares are as far as the indexer has folded, the
    /// amounts are those shares valued at this instant.
    ///
    /// RFC 3339, matching `sync.updated_at` and `pricing.updated_at`. The
    /// `as_of` **query parameter** that sets it is Unix seconds, so
    /// round-tripping this value means converting it.
    valued_at: String,

    /// How current the prices behind this page are — **a third clock**, beside
    /// `sync` and `valued_at`, because prices have their own source and their
    /// own cadence. Null when nothing here is priced, and null whenever `as_of`
    /// is set: amounts are extrapolated to that instant and prices are not, so a
    /// value mixing the two would be a number that never existed.
    pricing: Option<Pricing>,

    items: Vec<Item>,

    /// Hand back verbatim as `cursor` to fetch the next page. `null` is the last
    /// page. Opaque and signed: it is only valid for the listing that issued it,
    /// so changing the wallet, chain or Spoke filter while reusing it is refused
    /// rather than silently resuming somewhere else.
    next_cursor: Option<String>,
}

/// How far the indexer has got on this chain.
#[derive(Debug, Serialize)]
struct Progress {
    /// The last block the indexer processed on this chain.
    last_block: u64,

    /// The hash the indexer saw at that height.
    last_block_hash: String,

    /// When the indexer last advanced, by the database clock that recorded it.
    updated_at: String,

    /// Seconds since the indexer last advanced. Measured on the server that
    /// wrote the timestamp, so it is not affected by clock skew between hosts.
    age_seconds: u64,

    /// Whether `age_seconds` has passed this deployment's freshness threshold. A
    /// stale response is still served — the numbers were true as of
    /// `last_block` — but they are behind the chain.
    stale: bool,
}

/// How current the prices behind this page are.
#[derive(Debug, Serialize)]
struct Pricing {
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
struct Item {
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
fn instant(at: OffsetDateTime) -> Result<String, time::error::Format> {
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
fn instant_at(seconds: u64) -> Result<String, time::Error> {
    // The same fault an out-of-range timestamp produces, a century early.
    let seconds = i64::try_from(seconds).map_err(|_| time::error::ConversionRange)?;

    Ok(instant(OffsetDateTime::from_unix_timestamp(seconds)?)?)
}

type Labels = HashMap<Address, TokenLabel>;

/// Prices, keyed as the store hands them back.
type Prices = HashMap<ReserveKey, ReservePrice>;

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

#[cfg(test)]
mod tests {
    //! The route end to end, with the four ports doubled.
    //!
    //! One case asserts a whole body and the rest assert one decision each: the
    //! key spellings, the field order and the timestamp format are one contract
    //! and belong in one literal.

    use std::collections::HashMap;
    use std::sync::atomic::Ordering;

    use aave_positions::store::{Position, PositionAsset, PositionKey, PositionPage};
    use aave_positions::valuation::Valuation;
    use alloy_primitives::{Address, I256, U256, uint};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use indexing::SyncStatus;
    use prices::{ReserveKey, ReservePrice};
    use time::OffsetDateTime;
    use token_metadata::TokenLabel;

    use crate::test_support::{Stores, VALUED_AT, get, synced};

    const ALICE: &str = "0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8";
    const SPOKE: &str = "0x94e7a5dcbe816e498b89ab752661904e2f56c485";
    const OTHER_SPOKE: &str = "0x973a023a77420ba610f06b3858ad991df6d85a08";
    const HUB: &str = "0xcca852bc40e560adc3b1cc58ca5b55638ce826c9";
    const USDC: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";

    const RAY: U256 = uint!(1_000_000_000_000_000_000_000_000_000_U256);

    fn address(hex: &str) -> Address {
        hex.parse().expect("a literal address")
    }

    /// 1000 USDC supplied, nothing borrowed, on the Main Spoke's reserve 7.
    fn held() -> Position {
        Position {
            chain_id: 1,
            user: address(ALICE),
            spoke: address(SPOKE),
            reserve_id: U256::from(7),
            supplied_shares: I256::unchecked_from(500_500_000),
            drawn_shares: I256::ZERO,
            premium_shares: I256::ZERO,
            premium_offset_ray: I256::ZERO,
            net_supplied_amount: I256::unchecked_from(500_500_000),
            net_borrowed_amount: I256::ZERO,
            using_as_collateral: true,
            events: 3,
            asset: Some(PositionAsset {
                asset_id: U256::from(7),
                hub: address(HUB),
                underlying: address(USDC),
                decimals: 6,
            }),
            value: Some(Valuation {
                supplied_amount: U256::from(1_000_000_000u64),
                drawn_debt: U256::ZERO,
                premium_debt: U256::ZERO,
                total_debt: U256::ZERO,
                drawn_index: RAY,
            }),
        }
    }

    /// $0.99971505, as §7.4.3 records it, read 41 seconds ago.
    fn priced(spoke: &str, age_seconds: u64) -> (ReserveKey, ReservePrice) {
        (
            ReserveKey {
                spoke: address(spoke),
                reserve_id: U256::from(7),
            },
            ReservePrice {
                price: U256::from(99_971_505u64),
                priced_at: OffsetDateTime::from_unix_timestamp(
                    i64::try_from(VALUED_AT - age_seconds).expect("a representable instant"),
                )
                .expect("a representable instant"),
                age_seconds,
            },
        )
    }

    fn labelled() -> HashMap<Address, TokenLabel> {
        HashMap::from([(
            address(USDC),
            TokenLabel {
                symbol: Some("USDC".to_owned()),
                name: Some("USD Coin".to_owned()),
            },
        )])
    }

    fn request(query: &str) -> Request<Body> {
        Request::builder()
            .uri(format!("/api/v1/chains/1/users/{ALICE}/positions{query}"))
            .body(Body::empty())
            .expect("a well-formed request")
    }

    async fn answer(stores: Stores, query: &str) -> (StatusCode, String) {
        let (status, body, _) = get(stores.handler(), request(query)).await;

        (status, body)
    }

    #[tokio::test]
    async fn serves_the_whole_page_in_the_spelling_this_port_publishes() {
        // The one literal: key spellings, field order and both timestamps.
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: None,
            },
            labels: labelled(),
            prices: HashMap::from([priced(SPOKE, 41)]),
            ..Stores::default()
        };

        let (status, body) = answer(stores, "").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            concat!(
                r#"{"sync":{"last_block":25652535,"#,
                r#""last_block_hash":"0xabababababababababababababababababababababababababababababababab","#,
                r#""updated_at":"2026-07-25T17:21:11Z","age_seconds":7,"stale":false},"#,
                r#""valued_at":"2026-07-25T17:20:00Z","#,
                r#""pricing":{"updated_at":"2026-07-25T17:19:19Z","age_seconds":41,"stale":false},"#,
                r#""items":[{"chain_id":1,"#,
                r#""user":"0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8","#,
                r#""spoke":"0x94e7a5dcbe816e498b89ab752661904e2f56c485","#,
                r#""reserve_id":"7","supplied_shares":"500.5","drawn_shares":"0","#,
                r#""premium_shares":"0","premium_offset_ray":"0","#,
                r#""net_supplied_amount":"500.5","net_borrowed_amount":"0","#,
                r#""using_as_collateral":true,"events":3,"#,
                r#""asset":{"asset_id":"7","#,
                r#""hub":"0xcca852bc40e560adc3b1cc58ca5b55638ce826c9","#,
                r#""underlying":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","#,
                r#""decimals":6,"symbol":"USDC","name":"USD Coin"},"#,
                r#""value":{"supplied_amount":"1000","drawn_debt":"0","premium_debt":"0","#,
                r#""total_debt":"0","drawn_index":"1","price_usd":"0.99971505","#,
                r#""supplied_amount_usd":"999.71505","total_debt_usd":"0"}}],"#,
                r#""next_cursor":null}"#,
            )
        );
    }

    #[tokio::test]
    async fn a_chain_this_deployment_has_never_indexed_is_a_404_not_an_empty_page() {
        // `200` with nothing in it says the wallet holds no positions.
        let (status, body) = answer(
            Stores {
                sync: None,
                ..Stores::default()
            },
            "",
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            r#"{"message":"chain 1 has not been indexed by this deployment","error":"Not Found","status_code":404}"#
        );
    }

    #[tokio::test]
    async fn an_explicit_instant_skips_the_price_read_rather_than_discarding_it() {
        // Amounts extrapolate to that instant and prices do not. The counter
        // is what makes it a skip: a discarded result reads `1`.
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: 1_780_000_000,
                next: None,
            },
            labels: labelled(),
            prices: HashMap::from([priced(SPOKE, 41)]),
            ..Stores::default()
        };
        let reads = stores.price_reads.clone();

        let (status, body) = answer(stores, "?as_of=1780000000").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(reads.load(Ordering::Relaxed), 0);
        assert!(body.contains(r#""pricing":null"#), "{body}");
        assert!(body.contains(r#""price_usd":null"#), "{body}");
        assert!(body.contains(r#""supplied_amount_usd":null"#), "{body}");
        assert!(body.contains(r#""total_debt_usd":null"#), "{body}");
        assert!(
            body.contains(r#""valued_at":"2026-05-28T20:26:40Z""#),
            "{body}"
        );
    }

    #[tokio::test]
    async fn the_page_clock_is_the_oldest_price_behind_it_not_the_newest() {
        // Only diverges when the oracle refused a reserve and its last good
        // price was left to age, which is the case worth surfacing.
        let mut elsewhere = held();
        elsewhere.spoke = address(OTHER_SPOKE);

        let stores = Stores {
            page: PositionPage {
                items: vec![held(), elsewhere],
                valued_at: VALUED_AT,
                next: None,
            },
            labels: labelled(),
            prices: HashMap::from([priced(SPOKE, 41), priced(OTHER_SPOKE, 3_600)]),
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;

        assert!(
            body.contains(
                r#""pricing":{"updated_at":"2026-07-25T16:20:00Z","age_seconds":3600,"stale":true}"#
            ),
            "{body}"
        );
    }

    #[tokio::test]
    async fn an_unresolved_reserve_nulls_the_asset_the_value_and_the_shares_together() {
        // The scale lives on the asset: an unscaled integer in a field the
        // contract calls decimal is wrong by up to eighteen orders of magnitude.
        let mut unresolved = held();
        unresolved.asset = None;
        unresolved.value = None;
        unresolved.premium_offset_ray = I256::unchecked_from(-1);

        let stores = Stores {
            page: PositionPage {
                items: vec![unresolved],
                valued_at: VALUED_AT,
                next: None,
            },
            ..Stores::default()
        };

        let (status, body) = answer(stores, "").await;

        assert_eq!(status, StatusCode::OK);
        for null in [
            r#""supplied_shares":null"#,
            r#""drawn_shares":null"#,
            r#""premium_shares":null"#,
            r#""net_supplied_amount":null"#,
            r#""net_borrowed_amount":null"#,
            r#""asset":null"#,
            r#""value":null"#,
            r#""pricing":null"#,
        ] {
            assert!(body.contains(null), "{null} missing from {body}");
        }

        // A ray's scale is the protocol's fixed 27, so it survives.
        assert!(
            body.contains(r#""premium_offset_ray":"-0.000000000000000000000000001""#),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_reserve_nobody_has_priced_nulls_the_dollars_and_keeps_the_tokens() {
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: None,
            },
            labels: labelled(),
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;

        assert!(body.contains(r#""supplied_amount":"1000""#), "{body}");
        assert!(body.contains(r#""price_usd":null"#), "{body}");
        assert!(body.contains(r#""pricing":null"#), "{body}");
    }

    #[tokio::test]
    async fn a_token_enrichment_has_not_reached_serves_a_null_label() {
        // Absent from the map and present-with-null both serve null.
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: None,
            },
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;

        assert!(body.contains(r#""symbol":null,"name":null"#), "{body}");
    }

    #[tokio::test]
    async fn hands_back_a_cursor_it_will_take_again() {
        // Through the router rather than the codec.
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: Some(PositionKey {
                    spoke: address(SPOKE),
                    reserve_id: U256::from(7),
                }),
            },
            labels: labelled(),
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;
        let page: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
        let cursor = page["next_cursor"].as_str().expect("a cursor").to_owned();

        let (status, _) = answer(Stores::default(), &format!("?cursor={cursor}")).await;

        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn refuses_a_cursor_issued_for_a_different_listing() {
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: Some(PositionKey {
                    spoke: address(SPOKE),
                    reserve_id: U256::from(7),
                }),
            },
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;
        let page: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
        let cursor = page["next_cursor"].as_str().expect("a cursor").to_owned();

        // Well-formed, genuinely ours, and a listing it was not issued for.
        let (status, body) = answer(
            Stores::default(),
            &format!("?cursor={cursor}&spoke={SPOKE}"),
        )
        .await;

        // The whole body, because the demotion to a 400 and the `invalid page
        // cursor:` prefix are this layer's — `cursor` only says `Signature`.
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            concat!(
                r#"{"message":"invalid page cursor: signature does not match this listing","#,
                r#""error":"Bad Request","status_code":400}"#
            )
        );
    }

    #[tokio::test]
    async fn a_bad_parameter_is_a_400_through_the_router_and_not_only_in_the_parser() {
        let handler = Stores::default().handler();
        let request = Request::builder()
            .uri("/api/v1/chains/1/users/0xnope/positions")
            .body(Body::empty())
            .expect("a well-formed request");

        let (status, body, _) = get(handler, request).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body.contains("user must be a 20-byte hex address"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn is_mounted_under_the_prefix_and_nowhere_else() {
        // The prefix is configuration; what is worth pinning is that the route
        // is not also reachable without it.
        let handler = Stores::default().handler();
        let request = Request::builder()
            .uri(format!("/v1/chains/1/users/{ALICE}/positions"))
            .body(Body::empty())
            .expect("a well-formed request");

        let (status, _, _) = get(handler, request).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stamps_a_page_stale_when_the_indexer_has_stopped_advancing() {
        let stores = Stores {
            sync: Some(SyncStatus {
                age_seconds: 61,
                ..synced()
            }),
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;

        assert!(body.contains(r#""age_seconds":61,"stale":true"#), "{body}");
    }

    mod instants {
        //! The one spelling of a timestamp, and the two shapes it is not.

        use time::OffsetDateTime;

        use crate::positions::list::{instant, instant_at};

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
}
