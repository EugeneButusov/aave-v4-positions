//! What comes back, and what it means.
//!
//! One row of the join, and the walk from it to a [`Position`]. Every wide
//! value arrives as a decimal string — `toString` in the projection and a
//! 256-bit integer here are the two halves of keeping a balance past 2^53
//! exact (§7.5) — so this is also where a corrupt fold would surface.

use alloy_primitives::{Address, I256, U256};
use clickhouse_client::clickhouse;
use serde::Deserialize;

use super::{Error, Position, PositionAsset};
use crate::valuation::{AssetState, PositionShares, Valuation};

/// One row as ClickHouse renders it.
///
/// The nullability is the server's, not a guess — `DESCRIBE` over the statement
/// reports exactly this. It matters that `asset_id` and `hub` are **not**
/// nullable: a `LEFT JOIN` miss fills a non-nullable column with its default,
/// so an unresolved reserve arrives as `"0"` and `""` rather than as nothing.
/// What actually says the join missed is `underlying`, which is nullable in
/// `hub_assets_current` and therefore null when there is no right-hand row.
#[derive(Debug, clickhouse::Row, Deserialize)]
pub(super) struct Row {
    chain_id: u32,
    user: String,
    spoke: String,
    reserve_id: String,
    supplied_shares: String,
    drawn_shares: String,
    premium_shares: String,
    premium_offset_ray: String,
    net_supplied_amount: String,
    net_borrowed_amount: String,
    using_as_collateral: u8,
    events: i32,

    // The joined halves.
    asset_id: String,
    hub: String,
    underlying: Option<String>,
    decimals: Option<u8>,
    liquidity: String,
    added_shares: String,
    asset_drawn_shares: String,
    swept: String,
    asset_premium_shares: String,
    asset_premium_offset_ray: String,
    deficit_ray: String,
    realized_fees: Option<String>,
    liquidity_fee: Option<u16>,
    drawn_index: Option<String>,
    drawn_rate: Option<String>,
    checkpoint_at: Option<String>,
}

impl Row {
    /// The row as a position, valued at `at`.
    pub(super) fn position(&self, at: u64) -> Result<Position, Error> {
        Ok(Position {
            chain_id: self.chain_id,
            user: address("user", &self.user)?,
            spoke: address("spoke", &self.spoke)?,
            reserve_id: unsigned("reserve_id", &self.reserve_id)?,
            supplied_shares: signed("supplied_shares", &self.supplied_shares)?,
            drawn_shares: signed("drawn_shares", &self.drawn_shares)?,
            premium_shares: signed("premium_shares", &self.premium_shares)?,
            premium_offset_ray: signed("premium_offset_ray", &self.premium_offset_ray)?,
            net_supplied_amount: signed("net_supplied_amount", &self.net_supplied_amount)?,
            net_borrowed_amount: signed("net_borrowed_amount", &self.net_borrowed_amount)?,
            using_as_collateral: self.using_as_collateral == 1,
            events: self.events,
            asset: self.asset()?,
            value: self.value(at)?,
        })
    }

    /// The resolved reserve, or `None` if the registry has not seen it.
    fn asset(&self) -> Result<Option<PositionAsset>, Error> {
        let (Some(underlying), Some(decimals)) = (self.underlying.as_deref(), self.decimals) else {
            return Ok(None);
        };

        Ok(Some(PositionAsset {
            asset_id: unsigned("asset_id", &self.asset_id)?,
            hub: address("hub", &self.hub)?,
            underlying: address("underlying", underlying)?,
            decimals,
        }))
    }

    /// What the position is worth at `at`, or `None` if it cannot be said.
    ///
    /// **A negative share balance is `None` rather than a refusal.** Shares
    /// cannot go negative on chain, so one that has means the fold is wrong —
    /// and the row still reports the signed balance that says so. Valuing it is
    /// what cannot be done: `U256` holds no such number, and a negative debt is
    /// not something a caller can act on either.
    fn value(&self, at: u64) -> Result<Option<Valuation>, Error> {
        let (Some(state), Some(shares)) = (self.asset_state()?, self.shares()?) else {
            return Ok(None);
        };

        Ok(Some(shares.value_at(&state, at)?))
    }

    /// The Hub state a valuation needs, or `None` if any of it is missing.
    fn asset_state(&self) -> Result<Option<AssetState>, Error> {
        let (Some(checkpoint_index), Some(checkpoint_at)) =
            (self.drawn_index.as_deref(), self.checkpoint_at.as_deref())
        else {
            return Ok(None);
        };

        Ok(Some(AssetState {
            liquidity: unsigned("liquidity", &self.liquidity)?,
            added_shares: unsigned("added_shares", &self.added_shares)?,
            drawn_shares: unsigned("asset_drawn_shares", &self.asset_drawn_shares)?,
            swept: unsigned("swept", &self.swept)?,
            premium_shares: unsigned("asset_premium_shares", &self.asset_premium_shares)?,
            premium_offset_ray: signed("asset_premium_offset_ray", &self.asset_premium_offset_ray)?,
            deficit_ray: unsigned("deficit_ray", &self.deficit_ray)?,
            realized_fees: unsigned(
                "realized_fees",
                self.realized_fees.as_deref().unwrap_or("0"),
            )?,
            liquidity_fee: self.liquidity_fee.unwrap_or(0),
            checkpoint_index: unsigned("drawn_index", checkpoint_index)?,
            drawn_rate: rate(self.drawn_rate.as_deref().unwrap_or("0"))?,
            checkpoint_at: seconds("checkpoint_at", checkpoint_at)?,
        }))
    }

    fn shares(&self) -> Result<Option<PositionShares>, Error> {
        let (Some(supplied_shares), Some(drawn_shares), Some(premium_shares)) = (
            non_negative(signed("supplied_shares", &self.supplied_shares)?),
            non_negative(signed("drawn_shares", &self.drawn_shares)?),
            non_negative(signed("premium_shares", &self.premium_shares)?),
        ) else {
            return Ok(None);
        };

        Ok(Some(PositionShares {
            supplied_shares,
            drawn_shares,
            premium_shares,
            premium_offset_ray: signed("premium_offset_ray", &self.premium_offset_ray)?,
        }))
    }
}

fn non_negative(value: I256) -> Option<U256> {
    (!value.is_negative()).then(|| value.into_raw())
}

fn address(column: &'static str, value: &str) -> Result<Address, Error> {
    value.parse().map_err(|_| Error::Malformed {
        column,
        expected: "20-byte address",
        value: value.to_owned(),
    })
}

fn unsigned(column: &'static str, value: &str) -> Result<U256, Error> {
    U256::from_str_radix(value, 10).map_err(|_| Error::Malformed {
        column,
        expected: "uint256",
        value: value.to_owned(),
    })
}

fn signed(column: &'static str, value: &str) -> Result<I256, Error> {
    value.parse().map_err(|_| Error::Malformed {
        column,
        expected: "int256",
        value: value.to_owned(),
    })
}

fn rate(value: &str) -> Result<u128, Error> {
    value.parse().map_err(|_| Error::Malformed {
        column: "drawn_rate",
        expected: "uint96",
        value: value.to_owned(),
    })
}

fn seconds(column: &'static str, value: &str) -> Result<u64, Error> {
    value.parse().map_err(|_| Error::Malformed {
        column,
        expected: "unix seconds",
        value: value.to_owned(),
    })
}
