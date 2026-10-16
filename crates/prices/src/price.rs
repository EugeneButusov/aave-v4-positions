use alloy_primitives::{Address, U256};
use time::OffsetDateTime;

/// The key both sides of the join agree on. A type, where the TypeScript
/// builds a string — an [`Address`] has one representation and a `U256` one
/// value, so there is no spelling to get wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReserveKey {
    /// Keyed by reserve, not by token: `IAaveOracleV4` belongs to one Spoke, so
    /// the same ERC-20 on two Spokes has two prices that may disagree (§12.3).
    pub spoke: Address,
    pub reserve_id: U256,
}

/// A stored price, the moment it was read, and how long ago that was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservePrice {
    /// 8-decimal, per §7.4's `ORACLE_DECIMALS`.
    ///
    /// An integer rather than a decimal string: §7.1 multiplies it by an
    /// amount, and the wire format belongs to the DTO rather than the store.
    pub price: U256,

    /// Written by the database's clock, so freshness is judged against the
    /// server that wrote it. Not a block: the store is shared with whatever
    /// source lands next, and a market API has none to give.
    pub priced_at: OffsetDateTime,

    /// Whole seconds since this price was written, by the database's own clock.
    /// Not derivable from [`Self::priced_at`] by a reader: subtracting it from
    /// another process's clock reports skew as staleness, and a fast reader
    /// would mark a fresh price stale.
    pub age_seconds: u64,
}
