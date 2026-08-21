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

// The relaxation the valuation modules take: the arithmetic in a vector is the
// vector, and an instant a test names is clearer added than saturated.
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use alloy_primitives::{I256, U256};

    use crate::store::clickhouse::harness::{append, index, list_reserve, store};
    use crate::store::fixtures::{
        ALICE, At, CHAIN_ID, CHECKPOINT_AT, HUB, HUGE, RAY, SPOKE, USDC, YEAR, add_asset,
        add_reserve, ask, borrow, supply,
    };
    use crate::store::{PositionAsset, PositionQuery, PositionStore};

    /// The two LEFT JOINs, and what a miss on either one reports.
    mod the_registry {
        use super::{
            ALICE, At, HUB, I256, PositionAsset, PositionStore, U256, USDC, add_asset, add_reserve,
            append, ask, list_reserve, store, supply,
        };

        #[tokio::test]
        async fn resolves_a_reserve_to_its_hub_asset_and_token() {
            let (store, client) = store("resolves").await;
            list_reserve(&client, &[supply(At::block(200), ALICE, "7", "1000")]).await;

            // reserveId is a per-Spoke index and means nothing on its own (§1).
            // AddReserve gives it a Hub and an assetId; the Hub's AddAsset gives
            // that an ERC-20 and its decimals. Neither contract has both halves.
            let page = store.list(&ask()).await.unwrap();
            assert_eq!(
                page.items[0].asset,
                Some(PositionAsset {
                    asset_id: U256::from(7),
                    hub: HUB,
                    underlying: USDC,
                    decimals: 6,
                })
            );
        }

        #[tokio::test]
        async fn reports_none_rather_than_zero_for_a_reserve_it_has_never_seen() {
            let (store, client) = store("unregistered").await;
            append(
                &client,
                "spoke_events",
                &[supply(At::block(200), ALICE, "99", "1000")],
            )
            .await;

            // A zero here is indistinguishable from a real zero balance. The
            // position still appears, because its shares are real.
            let page = store.list(&ask()).await.unwrap();
            assert_eq!(page.items[0].supplied_shares, I256::try_from(1000).unwrap());
            assert_eq!(page.items[0].asset, None);
            assert_eq!(page.items[0].value, None);
        }

        #[tokio::test]
        async fn reports_none_when_the_hub_has_listed_the_asset_but_never_checkpointed_it() {
            let (store, client) = store("uncheckpointed").await;
            append(&client, "hub_events", &[add_asset(At::block(10), USDC, 6)]).await;
            append(
                &client,
                "spoke_events",
                &[
                    add_reserve(At::block(10), "7", "7", HUB),
                    supply(At::block(200), ALICE, "7", "1000"),
                ],
            )
            .await;

            // No UpdateAsset means no index, and without an index there is no
            // arithmetic to do — so no number is offered.
            assert_eq!(store.list(&ask()).await.unwrap().items[0].value, None);
        }
    }

    /// Decimal string in, integer out, and nothing lost on the way.
    mod the_columns {
        use super::{
            ALICE, At, CHAIN_ID, CHECKPOINT_AT, HUGE, PositionQuery, PositionStore, SPOKE, ask,
            borrow, index, list_reserve, store, supply,
        };

        #[tokio::test]
        async fn carries_a_share_balance_past_2_53_without_losing_its_tail() {
            let (store, client) = store("huge_shares").await;
            index(&client, &[supply(At::block(100), ALICE, "7", HUGE)]).await;

            // Not the JSON encoder's defaults: a share balance arriving as a
            // number has already lost its tail by the time it reaches this
            // process (§7.5). `toString` in the projection and a 256-bit
            // integer here are the two halves of keeping it.
            let page = store.list(&ask()).await.unwrap();
            assert_eq!(page.items[0].supplied_shares.to_string(), HUGE);
        }

        #[tokio::test]
        async fn reports_the_collateral_flag_and_the_event_count() {
            let (store, client) = store("flags").await;
            index(&client, &[supply(At::block(100), ALICE, "7", "500")]).await;

            let page = store.list(&ask()).await.unwrap();
            let position = &page.items[0];
            assert_eq!(position.chain_id, CHAIN_ID);
            assert_eq!(position.spoke, SPOKE);
            assert!(!position.using_as_collateral);
            assert_eq!(position.events, 1);
        }

        #[tokio::test]
        async fn carries_an_amount_past_2_53_without_losing_its_tail() {
            let (store, client) = store("huge_amount").await;
            list_reserve(&client, &[borrow(At::block(200), ALICE, "7", HUGE)]).await;

            let page = store
                .list(&PositionQuery {
                    as_of: Some(CHECKPOINT_AT),
                    ..ask()
                })
                .await
                .unwrap();

            let value = page.items[0].value.as_ref().unwrap();
            assert_eq!(value.drawn_debt.to_string(), HUGE);
        }
    }

    /// The row put through the valuation.
    mod the_amounts {
        use super::{
            ALICE, At, CHECKPOINT_AT, I256, PositionQuery, PositionStore, RAY, U256, YEAR, ask,
            borrow, list_reserve, store, supply,
        };

        #[tokio::test]
        async fn turns_supplied_shares_into_a_token_amount() {
            let (store, client) = store("supplied_amount").await;
            list_reserve(&client, &[supply(At::block(200), ALICE, "7", "1000")]).await;

            let page = store
                .list(&PositionQuery {
                    as_of: Some(CHECKPOINT_AT),
                    ..ask()
                })
                .await
                .unwrap();

            // Valued at the checkpoint itself, so the index has not moved: the
            // asset holds 1,000,000 shares against 1,000,000 of underlying, and
            // 1,000 shares redeem for 1,000.
            let value = page.items[0].value.as_ref().unwrap();
            assert_eq!(value.supplied_amount, U256::from(1000));
            assert_eq!(value.drawn_index.to_string(), RAY);
        }

        #[tokio::test]
        async fn grows_a_debt_with_time_on_a_fixed_share_balance() {
            let (store, client) = store("accrual").await;
            list_reserve(&client, &[borrow(At::block(200), ALICE, "7", "1000000")]).await;

            let now = store
                .list(&PositionQuery {
                    as_of: Some(CHECKPOINT_AT),
                    ..ask()
                })
                .await
                .unwrap();
            let later = store
                .list(&PositionQuery {
                    as_of: Some(CHECKPOINT_AT + YEAR),
                    ..ask()
                })
                .await
                .unwrap();

            // The whole reason a share balance is not a balance (§5): nothing
            // was indexed between these two reads.
            assert_eq!(
                now.items[0].value.as_ref().unwrap().total_debt,
                U256::from(1_000_000)
            );
            assert_eq!(
                later.items[0].value.as_ref().unwrap().total_debt,
                U256::from(1_050_000)
            );
            assert_eq!(now.items[0].drawn_shares, later.items[0].drawn_shares);
        }

        #[tokio::test]
        async fn keeps_the_shares_and_the_flow_beside_the_amount() {
            let (store, client) = store("cost_basis").await;
            list_reserve(&client, &[supply(At::block(200), ALICE, "7", "1000")]).await;

            // Cost basis and current value answer different questions, and the
            // difference between them is interest — so neither replaces the
            // other.
            let page = store.list(&ask()).await.unwrap();
            let position = &page.items[0];
            assert_eq!(position.supplied_shares, I256::try_from(1000).unwrap());
            assert_eq!(position.net_supplied_amount, I256::try_from(1000).unwrap());
            assert!(position.value.is_some());
        }
    }
}
