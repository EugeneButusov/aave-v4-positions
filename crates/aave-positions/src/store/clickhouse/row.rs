//! What comes back, and what it means.
//!
//! One row of the join, and the walk from it to a [`Position`]. Every wide
//! value arrives as a decimal string — `toString` in the projection and a
//! 256-bit integer here are the two halves of keeping a balance past 2^53
//! exact (§7.5) — so this is also where a corrupt fold would surface.

use alloy_primitives::{Address, I256, U256};
use clickhouse_client::clickhouse;
use serde::Deserialize;

use crate::store::{Error, Position, PositionAsset};
use crate::valuation::{AssetState, PositionShares, Valuation};

/// One row as ClickHouse renders it.
///
/// The nullability is the server's, not a guess — `DESCRIBE` over the statement
/// reports exactly this. It matters that `asset_id` and `hub` are **not**
/// nullable: a `LEFT JOIN` miss fills a non-nullable column with its default,
/// so an unresolved reserve arrives as `"0"` and `""` rather than as nothing.
/// What actually says the join missed is `underlying`, which is nullable in
/// `hub_assets_current` and therefore null when there is no right-hand row.
#[derive(clickhouse::Row, Deserialize)]
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
    pub(super) fn to_position(&self, at: u64) -> Result<Position, Error> {
        Ok(Position {
            chain_id: self.chain_id,
            user: parse_address("user", &self.user)?,
            spoke: parse_address("spoke", &self.spoke)?,
            reserve_id: parse_unsigned("reserve_id", &self.reserve_id)?,
            supplied_shares: parse_signed("supplied_shares", &self.supplied_shares)?,
            drawn_shares: parse_signed("drawn_shares", &self.drawn_shares)?,
            premium_shares: parse_signed("premium_shares", &self.premium_shares)?,
            premium_offset_ray: parse_signed("premium_offset_ray", &self.premium_offset_ray)?,
            net_supplied_amount: parse_signed("net_supplied_amount", &self.net_supplied_amount)?,
            net_borrowed_amount: parse_signed("net_borrowed_amount", &self.net_borrowed_amount)?,
            using_as_collateral: self.using_as_collateral == 1,
            events: self.events,
            asset: self.to_asset()?,
            value: self.to_valuation(at)?,
        })
    }

    /// The resolved reserve, or `None` if the registry has not seen it.
    fn to_asset(&self) -> Result<Option<PositionAsset>, Error> {
        let (Some(underlying), Some(decimals)) = (self.underlying.as_deref(), self.decimals) else {
            return Ok(None);
        };

        Ok(Some(PositionAsset {
            asset_id: parse_unsigned("asset_id", &self.asset_id)?,
            hub: parse_address("hub", &self.hub)?,
            underlying: parse_address("underlying", underlying)?,
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
    fn to_valuation(&self, at: u64) -> Result<Option<Valuation>, Error> {
        let (Some(state), Some(shares)) = (self.to_asset_state()?, self.to_shares()?) else {
            return Ok(None);
        };

        Ok(Some(shares.value_at(&state, at)?))
    }

    /// The Hub state a valuation needs, or `None` if any of it is missing.
    fn to_asset_state(&self) -> Result<Option<AssetState>, Error> {
        let (Some(checkpoint_index), Some(checkpoint_at)) =
            (self.drawn_index.as_deref(), self.checkpoint_at.as_deref())
        else {
            return Ok(None);
        };

        Ok(Some(AssetState {
            liquidity: parse_unsigned("liquidity", &self.liquidity)?,
            added_shares: parse_unsigned("added_shares", &self.added_shares)?,
            drawn_shares: parse_unsigned("asset_drawn_shares", &self.asset_drawn_shares)?,
            swept: parse_unsigned("swept", &self.swept)?,
            premium_shares: parse_unsigned("asset_premium_shares", &self.asset_premium_shares)?,
            premium_offset_ray: parse_signed(
                "asset_premium_offset_ray",
                &self.asset_premium_offset_ray,
            )?,
            deficit_ray: parse_unsigned("deficit_ray", &self.deficit_ray)?,
            realized_fees: parse_unsigned(
                "realized_fees",
                self.realized_fees.as_deref().unwrap_or("0"),
            )?,
            liquidity_fee: self.liquidity_fee.unwrap_or(0),
            checkpoint_index: parse_unsigned("drawn_index", checkpoint_index)?,
            drawn_rate: parse_rate(self.drawn_rate.as_deref().unwrap_or("0"))?,
            checkpoint_at: parse_seconds("checkpoint_at", checkpoint_at)?,
        }))
    }

    fn to_shares(&self) -> Result<Option<PositionShares>, Error> {
        let (Some(supplied_shares), Some(drawn_shares), Some(premium_shares)) = (
            non_negative(parse_signed("supplied_shares", &self.supplied_shares)?),
            non_negative(parse_signed("drawn_shares", &self.drawn_shares)?),
            non_negative(parse_signed("premium_shares", &self.premium_shares)?),
        ) else {
            return Ok(None);
        };

        Ok(Some(PositionShares {
            supplied_shares,
            drawn_shares,
            premium_shares,
            premium_offset_ray: parse_signed("premium_offset_ray", &self.premium_offset_ray)?,
        }))
    }
}

fn non_negative(value: I256) -> Option<U256> {
    (!value.is_negative()).then(|| value.into_raw())
}

/// Not [`Address::parse_checksummed`]: the column is lower-cased by the
/// projection, so EIP-55 validation would reject what the fold stores. The
/// empty string a `LEFT JOIN` miss leaves in a non-nullable column does fail
/// here, which is what keeps an unresolved reserve from reporting the zero
/// address.
fn parse_address(column: &'static str, value: &str) -> Result<Address, Error> {
    value.parse::<Address>().map_err(|_| Error::Malformed {
        column,
        expected: "20-byte address",
        value: value.to_owned(),
    })
}

/// Radix 10, and `from_dec_str` below for the same reason: ruint's `FromStr`
/// honours a base prefix, so `0x10` would parse as 16 rather than fail.
fn parse_unsigned(column: &'static str, value: &str) -> Result<U256, Error> {
    U256::from_str_radix(value, 10).map_err(|_| Error::Malformed {
        column,
        expected: "uint256",
        value: value.to_owned(),
    })
}

fn parse_signed(column: &'static str, value: &str) -> Result<I256, Error> {
    I256::from_dec_str(value).map_err(|_| Error::Malformed {
        column,
        expected: "int256",
        value: value.to_owned(),
    })
}

fn parse_rate(value: &str) -> Result<u128, Error> {
    value.parse::<u128>().map_err(|_| Error::Malformed {
        column: "drawn_rate",
        expected: "uint96",
        value: value.to_owned(),
    })
}

fn parse_seconds(column: &'static str, value: &str) -> Result<u64, Error> {
    value.parse::<u64>().map_err(|_| Error::Malformed {
        column,
        expected: "unix seconds",
        value: value.to_owned(),
    })
}

/// The mapping, with no server in it. A `Row` is what ClickHouse sends; these
/// build one and say what it becomes.
///
/// Two of these are unreachable through a query: the fold cannot produce a
/// column that fails to parse, nor a negative share balance. What a `LEFT JOIN`
/// miss actually fills a column with is the other way round, and stays a live
/// case in [`super::super::position_store`].
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use alloy_primitives::{I256, U256};

    use super::Row;
    use crate::store::Error;
    use crate::store::fixtures::{ALICE, CHECKPOINT_AT, HUB, HUGE, RAY, SPOKE, USDC, YEAR};

    /// One RAY per share, a supply side of exactly 1e6 against 1e6, and nothing
    /// drawn — so every amount below is the share count and the arithmetic is
    /// not what is under test.
    fn row() -> Row {
        Row {
            chain_id: 1,
            user: ALICE.to_string().to_lowercase(),
            spoke: SPOKE.to_string().to_lowercase(),
            reserve_id: "7".to_owned(),
            supplied_shares: "1000".to_owned(),
            drawn_shares: "0".to_owned(),
            premium_shares: "0".to_owned(),
            premium_offset_ray: "0".to_owned(),
            net_supplied_amount: "1000".to_owned(),
            net_borrowed_amount: "0".to_owned(),
            using_as_collateral: 0,
            events: 1,

            asset_id: "7".to_owned(),
            hub: HUB.to_string().to_lowercase(),
            underlying: Some(USDC.to_string().to_lowercase()),
            decimals: Some(6),
            liquidity: "1000000".to_owned(),
            added_shares: "1000000".to_owned(),
            asset_drawn_shares: "0".to_owned(),
            swept: "0".to_owned(),
            asset_premium_shares: "0".to_owned(),
            asset_premium_offset_ray: "0".to_owned(),
            deficit_ray: "0".to_owned(),
            realized_fees: Some("0".to_owned()),
            liquidity_fee: Some(0),
            drawn_index: Some(RAY.to_owned()),
            drawn_rate: Some("0".to_owned()),
            checkpoint_at: Some(CHECKPOINT_AT.to_string()),
        }
    }

    /// The registry and the Hub, and what a column left empty reports.
    mod the_registry {
        use super::{ALICE, HUB, Row, U256, USDC, row};
        use crate::store::PositionAsset;

        #[test]
        fn resolves_the_reserve_from_the_joined_columns() {
            // reserveId is a per-Spoke index and means nothing on its own (§1).
            // AddReserve gives it a Hub and an assetId; the Hub's AddAsset gives
            // that an ERC-20 and its decimals. Neither contract has both halves.
            assert_eq!(
                row().to_position(0).unwrap().asset,
                Some(PositionAsset {
                    asset_id: U256::from(7),
                    hub: HUB,
                    underlying: USDC,
                    decimals: 6,
                })
            );
        }

        /// `underlying` is what says the join missed: it is nullable in
        /// `hub_assets_current`, where `asset_id` and `hub` are not and come
        /// back as their defaults.
        #[test]
        fn reports_no_asset_when_the_registry_has_not_resolved_the_reserve() {
            let unresolved = Row {
                asset_id: "0".to_owned(),
                hub: String::new(),
                underlying: None,
                decimals: None,
                ..row()
            };

            let position = unresolved.to_position(0).unwrap();
            assert_eq!(position.asset, None);
            assert_eq!(position.user, ALICE);
        }

        #[test]
        fn reports_no_value_when_the_hub_has_never_checkpointed() {
            // No UpdateAsset means no index, and without an index there is no
            // arithmetic to do — so no number is offered.
            let uncheckpointed = Row {
                drawn_index: None,
                checkpoint_at: None,
                ..row()
            };

            assert_eq!(uncheckpointed.to_position(0).unwrap().value, None);
        }
    }

    /// Decimal string in, integer out, and nothing lost on the way.
    mod the_columns {
        use super::{CHECKPOINT_AT, Error, HUGE, Row, row};

        #[test]
        fn carries_a_share_balance_past_2_53_without_losing_its_tail() {
            // A share balance arriving as a JSON number has already lost its
            // tail by the time it reaches this process (§7.5). `toString` in the
            // projection and a 256-bit integer here are the two halves of
            // keeping it.
            let huge = Row {
                supplied_shares: HUGE.to_owned(),
                ..row()
            };

            assert_eq!(
                huge.to_position(CHECKPOINT_AT)
                    .unwrap()
                    .supplied_shares
                    .to_string(),
                HUGE
            );
        }

        #[test]
        fn reports_the_collateral_flag_and_the_event_count() {
            let flagged = Row {
                using_as_collateral: 1,
                events: 4,
                ..row()
            };

            let position = flagged.to_position(CHECKPOINT_AT).unwrap();
            assert_eq!(position.chain_id, 1);
            assert!(position.using_as_collateral);
            assert_eq!(position.events, 4);
            assert!(
                !row()
                    .to_position(CHECKPOINT_AT)
                    .unwrap()
                    .using_as_collateral
            );
        }

        /// Unreachable from a fold the migrations built — `toInt256` refuses on
        /// the insert rather than storing something unparseable — so this is
        /// the only place the refusal is exercised at all.
        #[test]
        fn names_the_column_that_did_not_parse() {
            let corrupt = Row {
                drawn_shares: "not a number".to_owned(),
                ..row()
            };

            let refused = corrupt.to_position(CHECKPOINT_AT).unwrap_err();
            assert!(
                matches!(
                    refused,
                    Error::Malformed {
                        column: "drawn_shares",
                        expected: "int256",
                        ..
                    }
                ),
                "{refused:?}"
            );
            assert_eq!(
                refused.to_string(),
                "drawn_shares is not a int256: not a number"
            );
        }
    }

    /// The row put through the valuation.
    mod the_amounts {
        use super::{CHECKPOINT_AT, I256, RAY, Row, U256, YEAR, row};

        #[test]
        fn turns_supplied_shares_into_a_token_amount() {
            // Valued at the checkpoint itself, so the index has not moved: the
            // asset holds 1,000,000 shares against 1,000,000 of underlying, and
            // 1,000 shares redeem for 1,000.
            let value = row().to_position(CHECKPOINT_AT).unwrap().value.unwrap();

            assert_eq!(value.supplied_amount, U256::from(1000));
            assert_eq!(value.drawn_index.to_string(), RAY);
        }

        #[test]
        fn grows_a_debt_with_time_on_a_fixed_share_balance() {
            // 5% per annum, RAY-scaled, as `drawnRate` arrives on `UpdateAsset`.
            let borrower = Row {
                drawn_shares: "1000000".to_owned(),
                asset_drawn_shares: "1000000".to_owned(),
                drawn_rate: Some("50000000000000000000000000".to_owned()),
                ..row()
            };

            // The whole reason a share balance is not a balance (§5): the row
            // is the same one both times.
            let now = borrower.to_position(CHECKPOINT_AT).unwrap();
            let later = borrower.to_position(CHECKPOINT_AT + YEAR).unwrap();

            assert_eq!(now.value.unwrap().total_debt, U256::from(1_000_000));
            assert_eq!(later.value.unwrap().total_debt, U256::from(1_050_000));
            assert_eq!(now.drawn_shares, later.drawn_shares);
        }

        #[test]
        fn keeps_the_shares_and_the_flow_beside_the_amount() {
            // Cost basis and current value answer different questions, and the
            // difference between them is interest — so neither replaces the
            // other.
            let position = row().to_position(CHECKPOINT_AT).unwrap();

            assert_eq!(position.supplied_shares, I256::try_from(1000).unwrap());
            assert_eq!(position.net_supplied_amount, I256::try_from(1000).unwrap());
            assert!(position.value.is_some());
        }

        /// Shares cannot go negative on chain, so no query can produce this —
        /// and a fold that has means the row is worth reporting anyway.
        #[test]
        fn refuses_to_value_a_negative_balance_but_still_reports_it() {
            let drifted = Row {
                supplied_shares: "-5".to_owned(),
                ..row()
            };

            let position = drifted.to_position(CHECKPOINT_AT).unwrap();
            assert_eq!(position.supplied_shares, I256::try_from(-5).unwrap());
            assert_eq!(position.value, None);
        }
    }
}
