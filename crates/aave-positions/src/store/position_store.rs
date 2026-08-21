//! Reads the folded positions out of ClickHouse.

use std::time::{SystemTime, UNIX_EPOCH};

use alloy_primitives::{Address, I256, U256};
// The derive emits a bare `clickhouse::` path, so the re-export has to be in
// scope under that name. Which is what the re-export is for: one dependency,
// and no way to end up on a different version than the client was built
// against.
use clickhouse_client::clickhouse::{self, Client};
use serde::Deserialize;

use super::{Error, Position, PositionAsset, PositionKey, PositionPage, PositionQuery};
use crate::valuation::{AssetState, PositionShares, Valuation};

/// One row as ClickHouse renders it: every wide integer already a string.
///
/// The nullability is the server's, not a guess — `DESCRIBE` over the query
/// below reports exactly this. It matters that `asset_id` and `hub` are **not**
/// nullable: a `LEFT JOIN` miss fills a non-nullable column with its default,
/// so an unresolved reserve arrives as `"0"` and `""` rather than as nothing.
/// What actually says the join missed is `underlying`, which is nullable in
/// `hub_assets_current` and therefore null when there is no right-hand row.
#[derive(Debug, clickhouse::Row, Deserialize)]
struct Row {
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

/// Everything above the `WHERE` the caller's filters go into.
const SELECT_HEAD: &str = r"
        SELECT
            p.chain_id                      AS chain_id,
            p.user                          AS user,
            p.spoke                         AS spoke,
            toString(p.reserve_id)          AS reserve_id,
            toString(p.supplied_shares)     AS supplied_shares,
            toString(p.drawn_shares)        AS drawn_shares,
            toString(p.premium_shares)      AS premium_shares,
            toString(p.premium_offset_ray)  AS premium_offset_ray,
            toString(p.net_supplied_amount) AS net_supplied_amount,
            toString(p.net_borrowed_amount) AS net_borrowed_amount,
            p.using_as_collateral           AS using_as_collateral,
            toInt32(p.events)               AS events,
            toString(r.asset_id)            AS asset_id,
            r.hub                           AS hub,
            a.underlying                    AS underlying,
            a.decimals                      AS decimals,
            toString(a.liquidity)           AS liquidity,
            toString(a.added_shares)        AS added_shares,
            toString(a.drawn_shares)        AS asset_drawn_shares,
            toString(a.swept)               AS swept,
            toString(a.premium_shares)      AS asset_premium_shares,
            toString(a.premium_offset_ray)  AS asset_premium_offset_ray,
            toString(a.deficit_ray)         AS deficit_ray,
            toString(a.realized_fees)       AS realized_fees,
            a.liquidity_fee                 AS liquidity_fee,
            toString(a.drawn_index)         AS drawn_index,
            toString(a.drawn_rate)          AS drawn_rate,
            toString(toUnixTimestamp(a.index_timestamp)) AS checkpoint_at
        FROM (
            SELECT *
            FROM user_positions_current
            WHERE ";

/// Everything below them.
const SELECT_TAIL: &str = r"
            ORDER BY user, spoke, reserve_id
            LIMIT {limit:UInt32}
        ) AS p
        -- **A join, not the UNION ALL the collateral flag got.** The two cases
        -- differ structurally, and EXPLAIN indexes = 1 shows how.
        --
        -- The left side prunes. Both branches of user_positions_current report
        -- PrimaryKey Keys: chain_id, user, spoke with the wallet predicate as
        -- their condition and Search Algorithm: binary search — that is the
        -- UNION ALL pushdown the flag was shaped for, doing its job.
        --
        -- The right sides do not, and cannot. All three read with
        -- PrimaryKey Condition: true — no predicate pushed in at all. The step
        -- is JOIN FillRightFirst: the right relation is built before a single
        -- left row is seen, so there is no key to filter by. A hash join
        -- materialises its whole right side by construction.
        --
        -- **UNION ALL would not change that.** Its advantage is exactly the
        -- pushdown above, and the Hub dimension has no predicate to push: it is
        -- keyed (chain, hub, asset_id), neither known until the registry
        -- resolves. It is not even expressible — UNION ALL needs one grouping
        -- key every branch shares, and the Hub dependency is transitive rather
        -- than co-keyed.
        --
        -- Reading the right side whole is fine when it *is* 17 rows, which is
        -- what the join was argued on. What is not fine is that producing those
        -- 17 reads 8 granules across 5 parts of hub_asset_state — the view
        -- collapses and argMaxes the lot on every page. Making that dimension a
        -- table is the fix; the README's 'Not here yet' says how.
        --
        -- LEFT, because a position must survive a reserve the registry has not
        -- seen. The nulls that produces are reported as nulls rather than zeros.
        LEFT JOIN spoke_reserves_current AS r
               ON r.chain_id = p.chain_id AND r.spoke = p.spoke AND r.reserve_id = p.reserve_id
        LEFT JOIN hub_assets_current AS a
               ON a.chain_id = r.chain_id AND a.hub = r.hub AND a.asset_id = r.asset_id
        -- Qualified, and it has to be. Unqualified, reserve_id binds to the
        -- toString alias above and sorts the decimal digits as text, putting 13
        -- before 3 — which then hands the next page a key from the wrong row.
        ORDER BY p.spoke, p.reserve_id";

/// Reads the folded positions out of ClickHouse.
///
/// Three shapes worth explaining, because all three are deliberate:
///
/// **`chain_id` and `user` are required and bound**, which is the leading pair
/// of the sorting key. That is what makes a page a seek into one wallet's
/// contiguous rows rather than a filter over everyone's. `spoke` narrows
/// further when it is given, and its absence costs nothing: the prefix that
/// does the seeking is already pinned without it.
///
/// **The resume point is the whole of what the prefix leaves free** —
/// `(spoke, reserve_id)`, compared as a pair even when `spoke` is pinned and
/// the first half is therefore constant. One comparison rather than two
/// branches: a `reserve_id`-only special case would silently read a resume
/// point from one Spoke against another's rows if the two ever disagreed.
///
/// **Every wide integer is `toString`-ed in SQL.** These columns are 256-bit
/// and the amounts routinely pass 2^53; a share balance that arrives as a
/// number has already lost its tail by the time it reaches this process (§7.5).
#[derive(Clone)]
pub struct ClickHousePositionStore {
    client: Client,
}

impl ClickHousePositionStore {
    /// Takes the client by value: `clickhouse::Client` is an `Arc` inside, so a
    /// caller sharing one clones it visibly.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// One wallet's open positions, valued at one instant.
    ///
    /// **Only open positions.** §12.1: a position exists while its shares are
    /// non-zero. A closed one keeps its row and its event count, and is
    /// filtered out here rather than deleted anywhere.
    pub async fn list(&self, query: &PositionQuery) -> Result<PositionPage, Error> {
        let mut filters = vec![
            // The leading pair of the sorting key, so the scan starts at this
            // wallet's rows rather than filtering its way to them.
            "chain_id = {chainId:UInt32}",
            "user = {user:String}",
            // Deliberately `!= 0` rather than §12.1's `> 0`. Shares cannot go
            // negative on chain, so a negative fold is drift — and it should
            // surface as a visibly wrong number for §9 to catch, not vanish
            // behind the filter that hides closed positions.
            "(supplied_shares != 0 OR drawn_shares != 0)",
        ];
        if query.spoke.is_some() {
            filters.push("spoke = {spoke:String}");
        }
        if query.after.is_some() {
            filters.push("(spoke, reserve_id) > ({afterSpoke:String}, {afterReserve:UInt256})");
        }

        let sql = format!("{SELECT_HEAD}{}{SELECT_TAIL}", filters.join(" AND "));

        let mut pending = self
            .client
            .query(&sql)
            .param("chainId", query.chain_id)
            // Fetch one more than asked. Its presence is what says there is a
            // next page; counting the whole result set to find out would defeat
            // the point of keyset paging.
            .param("limit", u64::from(query.limit).saturating_add(1))
            .param("user", lower_case(query.user));

        if let Some(spoke) = query.spoke {
            pending = pending.param("spoke", lower_case(spoke));
        }
        if let Some(after) = &query.after {
            pending = pending
                .param("afterSpoke", lower_case(after.spoke))
                .param("afterReserve", after.reserve_id.to_string());
        }

        let rows = pending.fetch_all::<Row>().await?;

        // One instant for the whole page, so two positions in one response
        // cannot disagree about what time it is — and reported back, because an
        // amount without the moment it was computed at is not reproducible
        // (§12.6).
        let valued_at = query.as_of.unwrap_or_else(now);

        let limit = usize::try_from(query.limit).unwrap_or(usize::MAX);
        let full = rows.len() > limit;
        let items = rows
            .iter()
            .take(limit)
            .map(|row| position(row, valued_at))
            .collect::<Result<Vec<_>, Error>>()?;

        let next = full
            .then(|| items.last())
            .flatten()
            .map(|last| PositionKey {
                spoke: last.spoke,
                reserve_id: last.reserve_id,
            });

        Ok(PositionPage {
            items,
            valued_at,
            next,
        })
    }
}

/// Lower-cased, because the fold stores addresses that way and a caller reading
/// a checksummed one off a block explorer should not have to know that.
fn lower_case(address: Address) -> String {
    address.to_string().to_lowercase()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn position(row: &Row, at: u64) -> Result<Position, Error> {
    Ok(Position {
        chain_id: row.chain_id,
        user: address("user", &row.user)?,
        spoke: address("spoke", &row.spoke)?,
        reserve_id: unsigned("reserve_id", &row.reserve_id)?,
        supplied_shares: signed("supplied_shares", &row.supplied_shares)?,
        drawn_shares: signed("drawn_shares", &row.drawn_shares)?,
        premium_shares: signed("premium_shares", &row.premium_shares)?,
        premium_offset_ray: signed("premium_offset_ray", &row.premium_offset_ray)?,
        net_supplied_amount: signed("net_supplied_amount", &row.net_supplied_amount)?,
        net_borrowed_amount: signed("net_borrowed_amount", &row.net_borrowed_amount)?,
        using_as_collateral: row.using_as_collateral == 1,
        events: row.events,
        asset: asset(row)?,
        value: value(row, at)?,
    })
}

/// The resolved reserve, or `None` if the registry has not seen it.
fn asset(row: &Row) -> Result<Option<PositionAsset>, Error> {
    let (Some(underlying), Some(decimals)) = (row.underlying.as_deref(), row.decimals) else {
        return Ok(None);
    };

    Ok(Some(PositionAsset {
        asset_id: unsigned("asset_id", &row.asset_id)?,
        hub: address("hub", &row.hub)?,
        underlying: address("underlying", underlying)?,
        decimals,
    }))
}

/// The Hub state a valuation needs, or `None` if any of it is missing.
fn asset_state(row: &Row) -> Result<Option<AssetState>, Error> {
    let (Some(checkpoint_index), Some(checkpoint_at)) =
        (row.drawn_index.as_deref(), row.checkpoint_at.as_deref())
    else {
        return Ok(None);
    };

    Ok(Some(AssetState {
        liquidity: unsigned("liquidity", &row.liquidity)?,
        added_shares: unsigned("added_shares", &row.added_shares)?,
        drawn_shares: unsigned("asset_drawn_shares", &row.asset_drawn_shares)?,
        swept: unsigned("swept", &row.swept)?,
        premium_shares: unsigned("asset_premium_shares", &row.asset_premium_shares)?,
        premium_offset_ray: signed("asset_premium_offset_ray", &row.asset_premium_offset_ray)?,
        deficit_ray: unsigned("deficit_ray", &row.deficit_ray)?,
        realized_fees: unsigned("realized_fees", row.realized_fees.as_deref().unwrap_or("0"))?,
        liquidity_fee: row.liquidity_fee.unwrap_or(0),
        checkpoint_index: unsigned("drawn_index", checkpoint_index)?,
        drawn_rate: rate(row.drawn_rate.as_deref().unwrap_or("0"))?,
        checkpoint_at: seconds("checkpoint_at", checkpoint_at)?,
    }))
}

/// What the position is worth at `at`, or `None` if it cannot be said.
///
/// **A negative share balance is `None` rather than a refusal.** Shares cannot
/// go negative on chain, so one that has means the fold is wrong — and the row
/// still reports the signed balance that says so. Valuing it is what cannot be
/// done: `U256` holds no such number, and a negative debt is not something a
/// caller can act on either.
fn value(row: &Row, at: u64) -> Result<Option<Valuation>, Error> {
    let (Some(state), Some(shares)) = (asset_state(row)?, shares(row)?) else {
        return Ok(None);
    };

    Ok(Some(shares.value_at(&state, at)?))
}

fn shares(row: &Row) -> Result<Option<PositionShares>, Error> {
    let (Some(supplied_shares), Some(drawn_shares), Some(premium_shares)) = (
        non_negative(signed("supplied_shares", &row.supplied_shares)?),
        non_negative(signed("drawn_shares", &row.drawn_shares)?),
        non_negative(signed("premium_shares", &row.premium_shares)?),
    ) else {
        return Ok(None);
    };

    Ok(Some(PositionShares {
        supplied_shares,
        drawn_shares,
        premium_shares,
        premium_offset_ray: signed("premium_offset_ray", &row.premium_offset_ray)?,
    }))
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

// Same reason as the valuation modules': the arithmetic in a vector is the
// vector, and an instant a test names is clearer added than saturated.
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use super::super::fixtures::{
        ALICE, At, BOB, CHAIN_ID, Event, HUB, HUGE, RAY, ROUTER, SECOND_SPOKE, SPOKE, T0, USDC,
        add, add_asset, add_reserve, append, borrow, draw, migrated_database, revert, supplied_by,
        supply, update_asset, withdraw,
    };
    use super::*;

    /// The block every Hub fixture checkpoints at, and the instant it lands on.
    const CHECKPOINT_BLOCK: u64 = 100;
    const CHECKPOINT_AT: u64 = T0 + CHECKPOINT_BLOCK;
    const YEAR: u64 = 365 * 24 * 3600;
    /// 5% per annum, RAY-scaled, as `drawnRate` arrives on `UpdateAsset`.
    const FIVE_PERCENT: &str = "50000000000000000000000000";

    /// A database of this test's own, and a store over it.
    async fn store(test: &str) -> (ClickHousePositionStore, Client) {
        let client = migrated_database(&format!("rust_positions_{test}")).await;
        (ClickHousePositionStore::new(client.clone()), client)
    }

    /// The query every case varies from: this wallet, on this Spoke, now.
    fn ask() -> PositionQuery {
        PositionQuery {
            chain_id: CHAIN_ID,
            user: ALICE,
            spoke: Some(SPOKE),
            limit: 100,
            after: None,
            as_of: None,
        }
    }

    /// What the processor does with a dispatched range: cancel, then write.
    async fn index(client: &Client, from: u64, to: u64, batch: &[Event]) {
        revert(client, "spoke_events", from, to).await;
        append(client, "spoke_events", batch).await;
    }

    /// A reserve that resolves all the way to a token, and a Hub asset with a
    /// checkpoint — the state valuation needs before it can produce a number.
    ///
    /// Asset 7 borrows 400,000 of the 1,000,000 supplied, so the index actually
    /// accrues: the short-circuit would hold it at RAY if nothing were drawn.
    async fn list_reserve(client: &Client, events: &[Event]) {
        append(
            client,
            "hub_events",
            &[
                add_asset(At::block(10), USDC, 6),
                add(At::block(20), "1000000", "1000000"),
                draw(At::block(30), "400000", "400000"),
                update_asset(At::block(CHECKPOINT_BLOCK), RAY, FIVE_PERCENT),
            ],
        )
        .await;

        let mut spoke = vec![add_reserve(At::block(10), "7", "7", HUB)];
        spoke.extend_from_slice(events);
        append(client, "spoke_events", &spoke).await;
    }

    fn reserve_ids(page: &PositionPage) -> Vec<String> {
        page.items
            .iter()
            .map(|item| item.reserve_id.to_string())
            .collect()
    }

    mod scope {
        use super::*;

        #[tokio::test]
        async fn returns_one_wallet_on_one_spoke_and_nobody_else() {
            let (store, client) = store("one_wallet").await;
            index(
                &client,
                100,
                100,
                &[
                    supply(At::block(100), ALICE, "7", "500"),
                    supply(At::block(100).log(1), BOB, "7", "900"),
                ],
            )
            .await;

            // `user` is required rather than an optional filter: with `chain_id`
            // it is the leading pair of the sorting key, so a page is a seek
            // into contiguous rows rather than a scan.
            let page = store.list(&ask()).await.unwrap();
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].user, ALICE);
            assert_eq!(page.items[0].supplied_shares, I256::try_from(500).unwrap());
        }

        #[tokio::test]
        async fn matches_a_checksummed_address_against_the_lower_cased_fold() {
            let (store, client) = store("checksummed").await;
            index(
                &client,
                100,
                100,
                &[supply(At::block(100), ALICE, "7", "500")],
            )
            .await;

            // The caller reads a checksummed address off a block explorer; the
            // fold stores it lower-cased. `Address` renders checksummed, so
            // without the store lower-casing it this finds nothing.
            assert_eq!(
                ALICE.to_string(),
                "0x82D16fF1C724ab72F218A3f7f6DD3E5385ee87E8"
            );
            assert_eq!(store.list(&ask()).await.unwrap().items.len(), 1);
        }

        #[tokio::test]
        async fn finds_nothing_on_a_spoke_the_wallet_has_never_touched() {
            let (store, client) = store("untouched_spoke").await;
            index(
                &client,
                100,
                100,
                &[supply(At::block(100), ALICE, "7", "500")],
            )
            .await;

            let elsewhere = PositionQuery {
                spoke: Some(Address::repeat_byte(0x11)),
                ..ask()
            };
            assert!(store.list(&elsewhere).await.unwrap().items.is_empty());
        }

        #[tokio::test]
        async fn credits_the_user_never_the_caller_that_routed_for_them() {
            let (store, client) = store("router").await;
            index(
                &client,
                100,
                100,
                &[supplied_by(At::block(100), ROUTER, ALICE, "7", "500")],
            )
            .await;

            // §2: reading `caller` attributes large parts of the book to a
            // handful of position managers.
            assert_eq!(store.list(&ask()).await.unwrap().items.len(), 1);
            let routed = PositionQuery {
                user: ROUTER,
                ..ask()
            };
            assert!(store.list(&routed).await.unwrap().items.is_empty());
        }
    }

    mod across_spokes {
        use super::*;

        async fn seed_both(client: &Client) {
            index(
                client,
                100,
                100,
                &[
                    supply(At::block(100), ALICE, "7", "500"),
                    supply(At::block(100).log(1).on(SECOND_SPOKE), ALICE, "7", "900"),
                ],
            )
            .await;
        }

        #[tokio::test]
        async fn lists_every_spoke_when_none_is_named_each_row_saying_which() {
            let (store, client) = store("every_spoke").await;
            seed_both(&client).await;

            // The same `reserveId` on two Spokes is two positions, not one —
            // reserve ids are Spoke-scoped, so nothing here may be merged on
            // them. Every row carries its own Spoke precisely so a caller can
            // group rather than guess.
            let page = store
                .list(&PositionQuery {
                    spoke: None,
                    ..ask()
                })
                .await
                .unwrap();

            let seen: Vec<_> = page
                .items
                .iter()
                .map(|item| {
                    (
                        item.spoke,
                        item.reserve_id.to_string(),
                        item.supplied_shares,
                    )
                })
                .collect();
            assert_eq!(
                seen,
                vec![
                    (SPOKE, "7".to_owned(), I256::try_from(500).unwrap()),
                    (SECOND_SPOKE, "7".to_owned(), I256::try_from(900).unwrap()),
                ]
            );
        }

        #[tokio::test]
        async fn narrows_to_one_when_it_is_named() {
            let (store, client) = store("one_spoke").await;
            seed_both(&client).await;

            let page = store
                .list(&PositionQuery {
                    spoke: Some(SECOND_SPOKE),
                    ..ask()
                })
                .await
                .unwrap();

            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].spoke, SECOND_SPOKE);
            assert_eq!(page.items[0].supplied_shares, I256::try_from(900).unwrap());
        }

        #[tokio::test]
        async fn walks_a_page_boundary_that_falls_between_two_spokes() {
            let (store, client) = store("spoke_boundary").await;
            seed_both(&client).await;

            // The half of the resume point that only exists once `spoke` is
            // unpinned. A `reserve_id`-only key would resume at "> 7" and lose
            // the second Spoke's reserve 7 entirely, because it sorts after the
            // first Spoke's.
            let first = store
                .list(&PositionQuery {
                    spoke: None,
                    limit: 1,
                    ..ask()
                })
                .await
                .unwrap();
            assert_eq!(
                first.next,
                Some(PositionKey {
                    spoke: SPOKE,
                    reserve_id: U256::from(7)
                })
            );

            let second = store
                .list(&PositionQuery {
                    spoke: None,
                    limit: 1,
                    after: first.next,
                    ..ask()
                })
                .await
                .unwrap();
            assert_eq!(second.items.len(), 1);
            assert_eq!(second.items[0].spoke, SECOND_SPOKE);
            assert_eq!(second.items[0].reserve_id, U256::from(7));
            assert_eq!(second.next, None);
        }
    }

    mod what_counts_as_a_position {
        use super::*;

        #[tokio::test]
        async fn hides_a_closed_one_but_keeps_its_history() {
            let (store, client) = store("closed").await;
            index(
                &client,
                100,
                200,
                &[
                    supply(At::block(100), ALICE, "7", "500"),
                    withdraw(At::block(200), ALICE, "7", "500"),
                ],
            )
            .await;

            // §12.1: a position exists while its shares are non-zero. Its
            // history does not stop having happened, so the row and its event
            // count stay.
            assert!(store.list(&ask()).await.unwrap().items.is_empty());
            let events: i64 = client
                .query("SELECT sum(events) FROM user_positions")
                .fetch_one()
                .await
                .unwrap();
            assert_eq!(events, 2);
        }

        #[tokio::test]
        async fn keeps_a_debt_only_position_with_no_supply_behind_it() {
            let (store, client) = store("debt_only").await;
            index(
                &client,
                100,
                100,
                &[borrow(At::block(100), ALICE, "13", "400")],
            )
            .await;

            let page = store.list(&ask()).await.unwrap();
            assert_eq!(page.items.len(), 1);
            assert_eq!(page.items[0].reserve_id, U256::from(13));
            assert_eq!(page.items[0].supplied_shares, I256::ZERO);
            assert_eq!(page.items[0].drawn_shares, I256::try_from(400).unwrap());
        }
    }

    mod exactness {
        use super::*;

        #[tokio::test]
        async fn carries_a_share_balance_past_2_53_without_losing_its_tail() {
            let (store, client) = store("huge_shares").await;
            index(
                &client,
                100,
                100,
                &[supply(At::block(100), ALICE, "7", HUGE)],
            )
            .await;

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
            index(
                &client,
                100,
                100,
                &[supply(At::block(100), ALICE, "7", "500")],
            )
            .await;

            let page = store.list(&ask()).await.unwrap();
            let position = &page.items[0];
            assert_eq!(position.chain_id, CHAIN_ID);
            assert_eq!(position.spoke, SPOKE);
            assert!(!position.using_as_collateral);
            assert_eq!(position.events, 1);
        }
    }

    mod pagination {
        use super::*;

        /// Deliberately out of numeric order as text: 13 sorts before 3.
        const RESERVES: [&str; 5] = ["3", "7", "13", "21", "34"];

        async fn seed(client: &Client) {
            let batch: Vec<_> = RESERVES
                .iter()
                .enumerate()
                .map(|(index, reserve_id)| {
                    let log = u32::try_from(index).unwrap();
                    supply(At::block(100).log(log), ALICE, reserve_id, "500")
                })
                .collect();
            index(client, 100, 100, &batch).await;
        }

        #[tokio::test]
        async fn walks_every_position_exactly_once_in_numeric_order() {
            let (store, client) = store("walk").await;
            seed(&client).await;

            let mut seen = Vec::new();
            let mut after = None;
            loop {
                let page = store
                    .list(&PositionQuery {
                        limit: 2,
                        after,
                        ..ask()
                    })
                    .await
                    .unwrap();
                seen.extend(reserve_ids(&page));
                after = page.next;
                if after.is_none() {
                    break;
                }
            }

            // Keyset, so the boundary is a key rather than a row count: nothing
            // is returned twice or skipped even though the indexer is free to
            // write between pages. Ordered as UInt256, so 13 precedes 21 rather
            // than 3 — which is what an unqualified ORDER BY over the toString
            // alias got wrong.
            assert_eq!(seen, RESERVES);
        }

        #[tokio::test]
        async fn reports_no_next_key_when_the_page_is_not_full() {
            let (store, client) = store("last_page").await;
            seed(&client).await;

            let full = store
                .list(&PositionQuery { limit: 5, ..ask() })
                .await
                .unwrap();
            assert_eq!(full.next, None);

            let short = store
                .list(&PositionQuery { limit: 4, ..ask() })
                .await
                .unwrap();
            assert_eq!(
                short.next,
                Some(PositionKey {
                    spoke: SPOKE,
                    reserve_id: U256::from(21)
                })
            );
        }

        #[tokio::test]
        async fn resumes_after_the_row_the_key_names_not_before_it() {
            let (store, client) = store("resume").await;
            seed(&client).await;

            let first = store
                .list(&PositionQuery { limit: 2, ..ask() })
                .await
                .unwrap();
            let second = store
                .list(&PositionQuery {
                    limit: 2,
                    after: first.next.clone(),
                    ..ask()
                })
                .await
                .unwrap();

            assert_eq!(reserve_ids(&first), ["3", "7"]);
            assert_eq!(reserve_ids(&second), ["13", "21"]);
        }
    }

    mod the_registry {
        use super::*;

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

    mod the_amounts {
        use super::*;

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

        #[tokio::test]
        async fn values_every_position_on_a_page_at_one_instant() {
            let (store, client) = store("one_instant").await;
            list_reserve(
                &client,
                &[
                    supply(At::block(200), ALICE, "7", "1000"),
                    borrow(At::block(200).log(1), ALICE, "7", "500"),
                ],
            )
            .await;

            let page = store
                .list(&PositionQuery {
                    as_of: Some(CHECKPOINT_AT + YEAR),
                    ..ask()
                })
                .await
                .unwrap();

            // One instant for the whole page, reported back: an amount without
            // the moment it was computed at is not reproducible (§12.6).
            assert_eq!(page.valued_at, CHECKPOINT_AT + YEAR);
            let indexes: std::collections::BTreeSet<_> = page
                .items
                .iter()
                .map(|item| item.value.as_ref().map(|value| value.drawn_index))
                .collect();
            assert_eq!(indexes.len(), 1);
        }

        #[tokio::test]
        async fn defaults_to_now_when_no_instant_is_named() {
            let (store, client) = store("now").await;
            list_reserve(&client, &[supply(At::block(200), ALICE, "7", "1000")]).await;

            let before = now();
            let page = store.list(&ask()).await.unwrap();

            // Which is what the chain does — getUserDebt at `latest`
            // extrapolates to the head block rather than to the last event.
            assert!(page.valued_at >= before);
            assert_ne!(
                page.items[0]
                    .value
                    .as_ref()
                    .unwrap()
                    .drawn_index
                    .to_string(),
                RAY
            );
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
}
