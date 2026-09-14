//! What a page looks like on the wire.
//!
//! **Plain structs with no `rename_all`, which is the decision rather than the
//! absence of one.** Every multi-word key here is snake_case where the service
//! this replaces spells it camelCase — the same deviation `errors::json` makes
//! for `status_code` and the probe surface makes for `uptime_seconds`, applied
//! to the payload instead of to two keys nobody reads. This port reads as Rust
//! rather than as a transliteration; `docs/rust-migration.md` carries the rule
//! and the release note it earns.
//!
//! **These mirror the domain types rather than reusing them.** `store::Position`
//! is free to gain a field with the next ingestion increment; the wire contract
//! is not, and a shared type would move it without anyone deciding to.
//!
//! **Every number is a decimal string, already scaled**, for the reason
//! [`super::scale`] gives. **A field is null when the answer is unknown, never
//! zero** — a zero is indistinguishable from a real one, and §7.4's oracle
//! reverts rather than answer one.
//!
//! Field order is declaration order, and it is the order the service beside this
//! one emits. A gate comparing two responses reads it, so it is part of the
//! contract even where the spelling is not.

use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

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

    pub(crate) items: Vec<Position>,

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
    pub(crate) updated_at: String,

    /// Seconds since then, measured by the database clock rather than this
    /// process's, so clock skew between the two cannot be reported as staleness.
    pub(crate) age_seconds: u64,

    /// Whether that exceeds this deployment's threshold. It measures how long
    /// since **we** last read the oracle, never how long since a feed last
    /// moved — an hour without a Chainlink update is normal behaviour, and a
    /// threshold set from feed cadence would flag healthy feeds forever.
    pub(crate) stale: bool,
}

/// One user's stake in one reserve on one Spoke.
#[derive(Debug, Serialize)]
pub(crate) struct Position {
    pub(crate) chain_id: u32,

    /// The position owner, lower-cased. Never the caller that routed the action
    /// — position managers act on behalf of users, and crediting them would
    /// attribute large parts of the book to a handful of router addresses.
    pub(crate) user: String,

    /// The Spoke this position lives on. Always present, including when the
    /// request did not filter on one. Two Spokes are two isolated margin
    /// accounts with their own collateral factors, oracle and health factor, so
    /// positions on them may be listed together but must never be summed.
    pub(crate) spoke: String,

    /// The Spoke's own index for the reserve, and the only asset identity a
    /// position has today. Not a protocol-wide asset id and not a token address
    /// — no Spoke event carries one. The same id on two Spokes means two
    /// different things.
    pub(crate) reserve_id: String,

    /// Supplied balance, in shares, scaled by the asset's decimals. Not an asset
    /// amount. **Null exactly when `asset` is**: the scale lives on the asset,
    /// so without it there is no honest way to render this, and an unscaled
    /// integer in a field documented as decimal would be wrong by orders of
    /// magnitude.
    pub(crate) supplied_shares: Option<String>,

    /// Borrowed balance, in shares. Null on the same terms as `supplied_shares`.
    pub(crate) drawn_shares: Option<String>,

    /// Accrued risk premium, in shares. Null on the same terms.
    pub(crate) premium_shares: Option<String>,

    /// Premium offset, as a ray ratio. **Never null**, unlike the share fields
    /// beside it: a ray's scale is the protocol's fixed 27 rather than the
    /// asset's, so it can be rendered whether or not the registry has resolved
    /// the reserve.
    pub(crate) premium_offset_ray: String,

    /// Net principal supplied, in asset units. A *flow*, not a balance: it sums
    /// what the events carried, and between events the interest index accrues
    /// while emitting nothing. Null on the same terms as `supplied_shares`.
    pub(crate) net_supplied_amount: Option<String>,

    /// Net principal borrowed, in asset units. Also a flow.
    pub(crate) net_borrowed_amount: Option<String>,

    /// The user's own collateral flag, and only that. It is not the `collateral`
    /// position type, which additionally requires a non-zero collateral factor
    /// under the version of the reserve config the user is pinned to — config
    /// events this build does not ingest. Five of the Main Spoke's fourteen
    /// reserves sit at a zero factor, so this flag alone overstates collateral
    /// for those.
    pub(crate) using_as_collateral: bool,

    /// Ledger rows folded into this position. Never zero for a returned one.
    pub(crate) events: i32,

    /// What `reserve_id` actually refers to, once the registry and the Hub have
    /// both been read. Null with `value` when the join has nothing to offer.
    pub(crate) asset: Option<Asset>,

    /// The shares above, converted to whole tokens at `valued_at`. Null when
    /// `asset` is, and also when the Hub has listed the asset but not yet
    /// checkpointed its index — a zero there could not be told apart from a real
    /// zero balance.
    pub(crate) value: Option<Value>,
}

/// What a reserve refers to, once the registry and the Hub have both been read.
#[derive(Debug, Serialize)]
pub(crate) struct Asset {
    /// The Hub's id for the asset, which is what makes it comparable across
    /// Spokes.
    pub(crate) asset_id: String,

    /// The Hub holding the liquidity.
    pub(crate) hub: String,

    /// The ERC-20 itself. It appears in no Spoke event — resolving it needs the
    /// reserve registry and the Hub's own asset listing.
    pub(crate) underlying: String,

    /// Token decimals, as the Hub listed them.
    pub(crate) decimals: u8,

    /// The token's own `symbol()`. **A label, not an identity** — nothing stops
    /// two tokens claiming the same one. `underlying` is the identity. Null when
    /// the token has no `symbol()`, which ERC-20 permits, and also in the window
    /// before enrichment has reached a newly listed asset.
    pub(crate) symbol: Option<String>,

    /// The token's own `name()`. Null on the same terms as `symbol`.
    pub(crate) name: Option<String>,
}

/// What one position is worth at `valued_at`.
#[derive(Debug, Serialize)]
pub(crate) struct Value {
    /// Underlying redeemable for the supplied shares, in whole tokens, rounded
    /// down as the Hub does.
    pub(crate) supplied_amount: String,

    /// Principal debt, rounded up as the Spoke does.
    pub(crate) drawn_debt: String,

    /// Accrued risk premium, in whole tokens.
    pub(crate) premium_debt: String,

    /// `drawn_debt + premium_debt`.
    pub(crate) total_debt: String,

    /// The interest index this valuation used: the last checkpoint extrapolated
    /// to `valued_at`, as a ray ratio where `1` is no accrual. Published so a
    /// caller can reproduce the arithmetic rather than trust it.
    pub(crate) drawn_index: String,

    /// What Aave's own oracle prices one whole token at, in dollars. This is the
    /// protocol's view rather than the market's, which is the right one for a
    /// position: it is the number that drives liquidation. Null when the oracle
    /// has not been read for this reserve yet, or when `as_of` is set.
    pub(crate) price_usd: Option<String>,

    /// What `supplied_amount` is worth, in dollars. Every digit the protocol
    /// computed is kept — §7.1's unit puts `1e26` at one dollar, and this is
    /// that number divided rather than rounded, so it still reconciles against
    /// `getUserAccountData` exactly. Null on the same terms as `price_usd`.
    pub(crate) supplied_amount_usd: Option<String>,

    /// What `total_debt` is worth, in dollars. Note this prices the **rounded**
    /// token amount, which is what is owed and what a caller should display. The
    /// health factor is computed from an unrounded ray-scaled debt instead, so
    /// the two will differ in the last digits by design.
    pub(crate) total_debt_usd: Option<String>,
}

/// The wire's spelling of an instant: RFC 3339, in UTC, as `time` writes it.
///
/// **Not `Date.prototype.toISOString`**, which the service beside this one uses.
/// Measured: that emits exactly three fractional digits always and truncates
/// below them, so `…17.221456Z` goes out as `…17.221Z` and a whole second as
/// `….000Z`. `Rfc3339` trims trailing zeros and keeps the microseconds Postgres
/// actually stored. Same instant, valid ISO 8601 either way, and reproducing the
/// other shape would mean a hand-written format description whose only argument
/// is that JavaScript has one.
///
/// Forced to UTC rather than assumed: the driver hands back a `timestamptz` at
/// zero offset today, and a non-zero one would print `+02:00` where every reader
/// of this field expects `Z`.
///
/// # Errors
///
/// [`time::error::Format`], which needs an unrepresentable year to produce.
pub(crate) fn instant(at: OffsetDateTime) -> Result<String, time::error::Format> {
    at.to_offset(time::UtcOffset::UTC).format(&Rfc3339)
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

#[cfg(test)]
mod tests {
    use super::*;

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
