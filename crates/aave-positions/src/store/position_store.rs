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
use clickhouse_client::clickhouse::Client;

use super::row::Row;
use super::{Error, PositionKey, PositionPage, PositionQuery, sql};

#[cfg(test)]
mod tests;

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

    /// One wallet's open positions, valued at one instant.
    ///
    /// **Only open positions.** §12.1: a position exists while its shares are
    /// non-zero. A closed one keeps its row and its event count, and is
    /// filtered out here rather than deleted anywhere.
    pub async fn list(&self, query: &PositionQuery) -> Result<PositionPage, Error> {
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
