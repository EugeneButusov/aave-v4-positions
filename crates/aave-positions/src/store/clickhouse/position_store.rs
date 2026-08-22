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
        let (spoke_from, spoke_to) = sql::spoke_bounds(query.spoke);
        let pending = self
            .client
            .query(sql::STATEMENT)
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
            )
            .param("spokeFrom", spoke_from)
            .param("spokeTo", spoke_to);

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
    use alloy_primitives::I256;
    use clickhouse_client::clickhouse::Client;

    use super::ClickHousePositionStore;
    use crate::store::clickhouse::harness::{append, index, migrated_database, store};
    use crate::store::contract::{Fixture, Held, Listed, position_store_contract};
    use crate::store::fixtures::{
        ALICE, At, FIVE_PERCENT, HUB, RAY, ROUTER, T0, add, add_asset, add_reserve, ask, borrow,
        draw, supplied_by, supply, update_asset, withdraw,
    };
    use crate::store::{PositionQuery, PositionStore};

    /// This store, standing up its own database, as the contract wants it.
    struct ClickHouse {
        store: ClickHousePositionStore,
        client: Client,
    }

    impl Fixture for ClickHouse {
        type Store = ClickHousePositionStore;

        async fn fresh(case: &str) -> Self {
            let client = migrated_database(&format!("rust_contract_{case}")).await;
            Self {
                store: ClickHousePositionStore::new(client.clone()),
                client,
            }
        }

        fn store(&self) -> &Self::Store {
            &self.store
        }

        /// Each held position as the events that fold into it.
        ///
        /// The inversion the contract costs: it asks for a share balance and this has
        /// to produce a `Supply` and a `Borrow` that sum to one. Straightforward while
        /// the balances are positive, which is why
        /// `hides_a_position_whose_shares_have_netted_to_zero` stays a local case —
        /// a zero asked for here is a zero written, not a `+500` and a `-500`.
        async fn given_positions(&self, held: &[Held]) {
            let mut events = Vec::new();
            for (position, entry) in held.iter().enumerate() {
                let at = At::block(100)
                    .log(u32::try_from(position).unwrap().saturating_mul(2))
                    .on(entry.spoke);

                if entry.supplied_shares != "0" {
                    events.push(supply(
                        at,
                        entry.user,
                        entry.reserve_id,
                        entry.supplied_shares,
                    ));
                }
                if entry.drawn_shares != "0" {
                    events.push(borrow(
                        at.log(at.log.saturating_add(1)),
                        entry.user,
                        entry.reserve_id,
                        entry.drawn_shares,
                    ));
                }
            }
            append(&self.client, "spoke_events", &events).await;
        }

        /// A reserve resolved to a Hub asset, checkpointed and accruing.
        async fn given_reserve(&self, listed: &Listed) {
            let block = listed.checkpoint_at.saturating_sub(T0);
            append(
                &self.client,
                "hub_events",
                &[
                    add_asset(
                        At::block(10),
                        listed.asset_id,
                        listed.underlying,
                        listed.decimals,
                    ),
                    add(At::block(20), listed.asset_id, "1000000", "1000000"),
                    draw(At::block(30), listed.asset_id, "400000", "400000"),
                    update_asset(At::block(block), listed.asset_id, RAY, FIVE_PERCENT),
                ],
            )
            .await;
            append(
                &self.client,
                "spoke_events",
                &[add_reserve(
                    At::block(10).on(listed.spoke),
                    listed.reserve_id,
                    listed.asset_id,
                    HUB,
                )],
            )
            .await;
        }
    }

    position_store_contract!(ClickHouse);

    /// What the contract cannot ask for, because a [`Held`] has already lost
    /// it: the fold's own behaviour, observed through this store.
    mod the_fold {
        use super::*;

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
        async fn hides_a_position_whose_shares_have_netted_to_zero() {
            let (store, client) = store("closed").await;
            index(
                &client,
                &[
                    supply(At::block(100), ALICE, "7", "500"),
                    withdraw(At::block(200), ALICE, "7", "500"),
                ],
            )
            .await;

            // §12.1: a position exists while its shares are non-zero. That the
            // row and its event count survive being closed is the fold's, and
            // is asserted where the fold is.
            assert!(store.list(&ask()).await.unwrap().items.is_empty());
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
    }
}
