//! The statement one page is read with.
//!
//! **Two halves, and the seam is the query's own.** The inner seek is a
//! complete `SELECT` over one wallet's contiguous rows; the outer resolution
//! joins the registry and the Hub onto whatever it returned. Every server
//! parameter is in the seek, so the resolution is one literal and the seek is
//! the only thing assembled.
//!
//! No query builder: none of them speaks ClickHouse's `{name:Type}` parameters,
//! and this SQL has to stay diffable against `clickhouse-position-store.ts`
//! with the `EXPLAIN` findings beside the shapes they justify.

use alloy_primitives::Address;

/// The resolution, with `{seek}` where the paged subquery goes.
///
/// Neither slot is a server parameter — those carry a type, `{name:Type}` — and
/// both are substituted before the statement is sent.
const RESOLVE: &str = r"
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
{seek}
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

/// The seek: one wallet's contiguous rows, paged.
///
/// **`{narrowing}` is the only clause that is not always here, and that was
/// measured.** `spoke` narrows by equality when it is given and is absent when
/// it is not, because `({spoke:String} = '' OR spoke = {spoke:String})` — the
/// shape that would make this a single constant — prunes to the same granule
/// but drops `Search Algorithm: binary search` to `generic exclusion search`,
/// which is the property the resolution's comment cites as evidence.
///
/// **The resume key needs no slot at all.** `('', 0)` is below every real key,
/// since a Spoke address is never the empty string — so the beginning of the
/// listing is a value rather than a missing predicate. Measured: same
/// condition, same granule, same binary search as omitting it.
const SEEK: &str = r"            SELECT *
            FROM user_positions_current
            -- The leading pair of the sorting key, so the scan starts at this
            -- wallet's rows rather than filtering its way to them.
            WHERE chain_id = {chainId:UInt32}
              AND user = {user:String}
              -- Deliberately `!= 0` rather than §12.1's `> 0`. Shares cannot go
              -- negative on chain, so a negative fold is drift, and it should
              -- surface as a visibly wrong number for §9 to catch rather than
              -- vanish behind the filter that hides closed positions.
              AND (supplied_shares != 0 OR drawn_shares != 0)
              -- The whole of what the pinned prefix leaves free, compared as a
              -- pair even when the narrowing below pins the first half. One
              -- comparison rather than two branches: a reserve_id-only special
              -- case would silently read a resume point from one Spoke against
              -- another's rows if the two ever disagreed.
              AND (spoke, reserve_id) > ({afterSpoke:String}, {afterReserve:UInt256}){narrowing}
            ORDER BY user, spoke, reserve_id
            -- One more than asked. The extra row's presence is what says there
            -- is a next page; counting the whole result set to find out would
            -- defeat keyset paging.
            LIMIT {limit:UInt32}";

/// What fills `{narrowing}` when the caller named a Spoke.
const NARROWED_TO_ONE_SPOKE: &str = "\n              AND spoke = {spoke:String}";

/// The statement for one page.
pub(super) fn list(spoke: Option<Address>) -> String {
    let narrowing = if spoke.is_some() {
        NARROWED_TO_ONE_SPOKE
    } else {
        ""
    };

    RESOLVE
        .replace("{seek}", SEEK)
        .replace("{narrowing}", narrowing)
}

// The relaxation the valuation modules take: the arithmetic in a vector is the
// vector, and an instant a test names is clearer added than saturated.
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use alloy_primitives::{Address, I256, U256};

    use super::{RESOLVE, list};
    use crate::store::clickhouse::fixtures::{
        ALICE, At, BOB, RESERVES, ROUTER, SECOND_SPOKE, SPOKE, ask, borrow, index, reserve_ids,
        seed_both_spokes, seed_five_reserves, store, supplied_by, supply, withdraw,
    };
    use crate::store::{PositionKey, PositionQuery, PositionStore};

    /// The text, before any server sees it.
    mod the_statement {
        use super::{Address, RESOLVE, list};

        #[test]
        fn splices_the_seek_where_the_subquery_goes() {
            let statement = list(None);

            assert!(!statement.contains("{seek}"), "{statement}");
            assert!(!statement.contains("{narrowing}"), "{statement}");
            assert!(
                statement.contains("FROM (\n            SELECT *"),
                "{statement}"
            );
            assert!(
                statement.contains("LIMIT {limit:UInt32}\n        ) AS p"),
                "{statement}"
            );
        }

        #[test]
        fn narrows_to_one_spoke_only_when_one_is_named() {
            assert!(!list(None).contains("AND spoke = {spoke:String}"));
            assert!(list(Some(Address::ZERO)).contains("AND spoke = {spoke:String}"));
        }

        /// Every parameter is inside the seek, which is what lets the resolution be
        /// one literal rather than a second thing to assemble.
        #[test]
        fn the_resolution_binds_no_parameters() {
            assert!(!RESOLVE.replace("{seek}", "").contains(":UInt"));
            assert!(!RESOLVE.replace("{seek}", "").contains(":String"));
        }
    }

    /// The `WHERE` clause, one predicate at a time.
    mod which_rows_come_back {
        use super::{
            ALICE, Address, At, BOB, I256, PositionQuery, PositionStore, ROUTER, U256, ask, borrow,
            index, store, supplied_by, supply, withdraw,
        };

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

    /// What the optional `spoke` predicate narrows, and what it does not.
    mod across_spokes {
        use super::{
            I256, PositionKey, PositionQuery, PositionStore, SECOND_SPOKE, SPOKE, U256, ask,
            seed_both_spokes, store,
        };

        #[tokio::test]
        async fn lists_every_spoke_when_none_is_named_each_row_saying_which() {
            let (store, client) = store("every_spoke").await;
            seed_both_spokes(&client).await;

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
            seed_both_spokes(&client).await;

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
            seed_both_spokes(&client).await;

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

    /// The `ORDER BY`, and the key the next page starts after.
    mod order_and_resume {
        use super::{
            PositionQuery, PositionStore, RESERVES, ask, reserve_ids, seed_five_reserves, store,
        };

        #[tokio::test]
        async fn walks_every_position_exactly_once_in_numeric_order() {
            let (store, client) = store("walk").await;
            seed_five_reserves(&client).await;

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
        async fn resumes_after_the_row_the_key_names_not_before_it() {
            let (store, client) = store("resume").await;
            seed_five_reserves(&client).await;

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
}
