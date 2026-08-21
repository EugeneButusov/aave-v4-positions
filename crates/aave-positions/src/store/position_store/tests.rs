//! The store specs.
//!
//! Against a real ClickHouse, and **seeded through the fold rather than around
//! it**: these append decoded events and let the materialized views do the
//! work, which is what makes them store specs rather than assertions about a
//! hand-written table.
//!
//! Same relaxation the valuation modules take: the arithmetic in a vector is
//! the vector, and an instant a test names is clearer added than saturated.
#![allow(clippy::arithmetic_side_effects)]
use alloy_primitives::I256;

use super::*;
use crate::store::PositionAsset;
use crate::store::fixtures::{
    ALICE, At, BOB, CHAIN_ID, Event, HUB, HUGE, RAY, ROUTER, SECOND_SPOKE, SPOKE, T0, USDC, add,
    add_asset, add_reserve, append, borrow, draw, migrated_database, revert, supplied_by, supply,
    update_asset, withdraw,
};

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
