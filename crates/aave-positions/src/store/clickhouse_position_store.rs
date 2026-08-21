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
use super::{Error, PositionKey, PositionPage, PositionQuery, PositionStore, sql};

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
            .map(|row| row.position(valued_at))
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
    use super::*;
    use crate::store::fixtures::{
        ALICE, At, CHECKPOINT_AT, RAY, SPOKE, YEAR, ask, borrow, list_reserve, seed_five_reserves,
        store, supply,
    };

    /// The reason the port carries `#[async_trait]` rather than a plain
    /// `async fn`.
    ///
    /// A compile-time assertion and nothing else: `async fn` in a trait is
    /// stable and not dyn compatible, so without the macro this line is E0038
    /// and `bins/api` would have to take a generic parameter instead of the
    /// `Arc<dyn Trait>` the composition root is designed around. It needs no
    /// `Send + Sync` on the port — that is a separate requirement, and it
    /// arrives with the axum state that has it.
    #[test]
    fn the_port_is_held_as_a_trait_object() {
        fn held(store: ClickHousePositionStore) -> std::sync::Arc<dyn PositionStore> {
            std::sync::Arc::new(store)
        }

        let _ = held;
    }

    /// The extra row, and the key it turns into.
    mod where_a_page_stops {
        use super::*;

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
        use super::*;

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
}
