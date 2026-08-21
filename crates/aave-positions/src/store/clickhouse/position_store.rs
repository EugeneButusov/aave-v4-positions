//! Reads the folded positions out of ClickHouse.
//!
//! Three shapes worth explaining, because all three are deliberate:
//!
//! **`chain_id` and `user` are required and bound**, which is the leading pair
//! of the sorting key. That is what makes a page a seek into one wallet's
//! contiguous rows rather than a filter over everyone's. `spoke` narrows
//! further when it is given, and its absence costs nothing: the prefix that
//! does the seeking is already pinned without it.
//!
//! **The resume point is the whole of what the prefix leaves free** —
//! `(spoke, reserve_id)`, and `('', 0)` is the beginning of the listing rather
//! than a missing predicate. See [`super::sql`].
//!
//! **Every wide integer is `toString`-ed in SQL.** These columns are 256-bit
//! and the amounts routinely pass 2^53; a share balance that arrives as a
//! number has already lost its tail by the time it reaches this process (§7.5).

use std::time::{SystemTime, UNIX_EPOCH};

use alloy_primitives::{Address, U256};
use async_trait::async_trait;
use clickhouse_client::clickhouse::Client;

use super::row::Row;
use super::sql;
use crate::store::{Error, PositionKey, PositionPage, PositionQuery, PositionStore};

/// Reads the folded positions out of ClickHouse.
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
}

#[async_trait]
impl PositionStore for ClickHousePositionStore {
    async fn list(&self, query: &PositionQuery) -> Result<PositionPage, Error> {
        let after = query.after.as_ref();
        let mut pending = self
            .client
            .query(&sql::list(query.spoke))
            .param("chainId", query.chain_id)
            .param("user", lower_case(query.user))
            // One more than asked, so the extra row's presence is what says
            // there is a next page.
            .param("limit", u64::from(query.limit).saturating_add(1))
            // Absent means the beginning, and the beginning is a key: no Spoke
            // address is the empty string, so `('', 0)` is below every row.
            .param(
                "afterSpoke",
                after.map(|key| lower_case(key.spoke)).unwrap_or_default(),
            )
            .param(
                "afterReserve",
                after.map_or(U256::ZERO, |key| key.reserve_id).to_string(),
            );

        if let Some(spoke) = query.spoke {
            pending = pending.param("spoke", lower_case(spoke));
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
            .map(|row| row.to_position(valued_at))
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

// The relaxation the valuation modules take: the arithmetic in a vector is the
// vector, and an instant a test names is clearer added than saturated.
#[cfg(test)]
#[allow(clippy::arithmetic_side_effects)]
mod tests {
    use alloy_primitives::{Address, I256, U256};

    use super::{ClickHousePositionStore, now};
    use crate::store::clickhouse::harness::{
        append, index, list_reserve, seed_both_spokes, seed_five_reserves, store,
    };
    use crate::store::fixtures::{
        ALICE, At, BOB, CHECKPOINT_AT, RAY, RESERVES, ROUTER, SECOND_SPOKE, SPOKE, YEAR, ask,
        borrow, reserve_ids, supplied_by, supply, withdraw,
    };
    use crate::store::{PositionKey, PositionQuery, PositionStore};

    /// The reason the port carries `#[async_trait]` rather than a plain
    /// `async fn`.
    ///
    /// A compile-time assertion and nothing else: `async fn` in a trait is
    /// stable and not dyn compatible, so without the macro this line is E0038
    /// and `bins/api` would have to take a generic parameter instead of the
    /// `Arc<dyn Trait>` the composition root is designed around.
    #[test]
    fn the_port_is_held_as_a_trait_object() {
        fn held(store: ClickHousePositionStore) -> std::sync::Arc<dyn PositionStore> {
            std::sync::Arc::new(store)
        }

        let _ = held;
    }

    /// The extra row, and the key it turns into.
    mod where_a_page_stops {
        use super::{
            PositionKey, PositionQuery, PositionStore, SPOKE, U256, ask, seed_five_reserves, store,
        };

        #[tokio::test]
        async fn reports_no_next_key_when_the_page_is_not_full() {
            let (store, client) = store("last_page").await;
            seed_five_reserves(&client).await;

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
    }

    /// One time for the whole page, and what it is when nobody names it.
    mod the_instant {
        use super::{
            ALICE, At, CHECKPOINT_AT, PositionQuery, PositionStore, RAY, YEAR, ask, borrow,
            list_reserve, now, store, supply,
        };

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
            index(&client, &[supply(At::block(100), ALICE, "7", "500")]).await;

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
            index(&client, &[supply(At::block(100), ALICE, "7", "500")]).await;

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
            index(&client, &[borrow(At::block(100), ALICE, "13", "400")]).await;

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

    /// What a `LEFT JOIN` miss actually fills a column with, which only a real
    /// one can say.
    mod the_registry {
        use super::{ALICE, At, I256, PositionStore, append, ask, store, supply};

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
    }
}
