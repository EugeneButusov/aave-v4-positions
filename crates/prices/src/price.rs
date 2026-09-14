use alloy_primitives::{Address, U256};
use time::OffsetDateTime;

/// The key both sides of the join agree on.
///
/// **A type, where the TypeScript has a function.** There, a price is keyed by
/// `(spoke, reserveId)` and a `Map` cannot key on a pair, so `reserveKey()`
/// builds `${spoke.toLowerCase()}:${reserveId}` and its doc warns that "the
/// moment the store and the service each spell the key themselves is the moment
/// one of them forgets to lower-case the spoke and every price silently stops
/// joining". An [`Address`] has one representation and a `U256` one value, so
/// there is no spelling and nothing to forget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReserveKey {
    /// **Keyed by reserve, not by token.** `IAaveOracleV4` indexes by
    /// `reserveId` and belongs to one Spoke, so the same ERC-20 listed on two
    /// Spokes has two prices and they are allowed to disagree — each Spoke is
    /// an isolated margin account with its own oracle (§12.3).
    pub spoke: Address,
    pub reserve_id: U256,
}

/// A stored price, the moment it was read, and how long ago that was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservePrice {
    /// 8-decimal, per §7.4's `ORACLE_DECIMALS`.
    ///
    /// An integer rather than a decimal string: §7.1 multiplies this by an
    /// amount to reach a `Value` where `1e26` is one dollar, and the wire
    /// format belongs to the DTO rather than to the store — one parse instead
    /// of two, and one place for it to be wrong.
    pub price: U256,

    /// Written by the database's clock, so freshness is judged against the
    /// server that wrote it rather than the one reading it.
    ///
    /// **Not a block.** The read that produced this was pinned to one, but the
    /// store is shared with whatever source lands next and a market API has no
    /// block to give — see `011_reserve_prices.sql`.
    pub priced_at: OffsetDateTime,

    /// Whole seconds since this price was written, **measured by the database's
    /// own clock**.
    ///
    /// Not derivable from [`Self::priced_at`] by a reader: that timestamp is
    /// written by the database, so subtracting it from a different process's
    /// clock reports skew as staleness — and in the direction that matters, a
    /// fast reader declares a fresh price stale and a whole page's USD values
    /// suspect.
    pub age_seconds: u64,
}
