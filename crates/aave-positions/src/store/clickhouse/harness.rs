//! A migrated database, a store over it, and the events put into it.
//!
//! Not fixtures — those are [`crate::store::fixtures`], and they are data. This
//! is what stands a scenario up.
//!
//! **Seeded through the fold, not around it.** These append decoded events and
//! let the materialized views do the work, which is what makes the specs store
//! specs rather than assertions about a hand-written table. Writing into
//! `user_positions` directly would prove the SQL in this module and nothing
//! about the SQL it reads.
//!
//! The write path is reproduced here rather than imported: the event ledgers
//! belong to a crate that does not exist yet. It is two statements — an insert,
//! and a sign-flipped copy — and both are what the ledger's own migration
//! documents.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use clickhouse_client::clickhouse::{self, Client};
use clickhouse_client::{Config, build_client};
use serde::Serialize;

use super::ClickHousePositionStore;
use crate::store::fixtures::{
    ALICE, At, CHAIN_ID, CHECKPOINT_BLOCK, Event, FIVE_PERCENT, HUB, RAY, RESERVES, SECOND_SPOKE,
    USDC, add, add_asset, add_reserve, draw, supply, update_asset,
};

// --- the write path ---------------------------------------------------------

/// One row of either ledger. Both have the same shape, which is why the
/// TypeScript shares one write path between them.
#[derive(clickhouse::Row, Serialize)]
struct LedgerRow<'a> {
    chain_id: u32,
    address: String,
    block_number: u64,
    log_index: u32,
    version: u64,
    block_hash: &'a str,
    block_timestamp: u32,
    tx_hash: &'a str,
    tx_index: u32,
    event_name: &'a str,
    topic1: Option<&'a str>,
    topic2: Option<&'a str>,
    topic3: Option<&'a str>,
    body: &'a str,
    data: &'a str,
    sign: i8,
}

/// One stamp per batch. Half of the ledger's pairing key, so a row needs one
/// even where nothing here writes the other half.
fn next_version() -> u64 {
    static VERSION: AtomicU64 = AtomicU64::new(0);

    VERSION
        .fetch_add(1, Ordering::Relaxed)
        .saturating_add(seconds_now().saturating_mul(1_000))
}

fn seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Appends a batch to whichever ledger the events came from.
pub(super) async fn append(client: &Client, table: &str, events: &[Event]) {
    if events.is_empty() {
        return;
    }

    let version = next_version();
    let mut insert = client.insert::<LedgerRow<'_>>(table).await.unwrap();
    for event in events {
        insert
            .write(&LedgerRow {
                chain_id: CHAIN_ID,
                address: event.address.to_string().to_lowercase(),
                block_number: event.at.block,
                log_index: event.at.log,
                version,
                block_hash: "0xaaaa",
                block_timestamp: event.timestamp,
                tx_hash: "0xbbbb",
                tx_index: 0,
                event_name: event.name,
                // The projections read every field out of `body`; the topics
                // are the indexed copies and nothing here needs them.
                topic1: None,
                topic2: None,
                topic3: None,
                body: &event.body,
                data: "0x",
                sign: 1,
            })
            .await
            .unwrap();
    }
    insert.end().await.unwrap();
}

// --- the database -----------------------------------------------------------

fn config(database: &str) -> Config {
    Config {
        url: std::env::var("CLICKHOUSE_URL").unwrap_or_else(|_| "http://localhost:8123".to_owned()),
        database: database.to_owned(),
        user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
        password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
    }
}

/// A migrated database of the caller's own.
///
/// **One per test, not one per file.** `cargo` runs these concurrently against
/// one server, and every one of them writes the same table names — a shared
/// database has them folding each other's events into the page under
/// assertion. It also removes the `TRUNCATE` between tests, and with it the
/// question of whether the list of tables to truncate is complete.
///
/// **Dropped first, not `IF NOT EXISTS`.** Every migration is written
/// `IF NOT EXISTS`, so against a database left behind by an earlier run the
/// whole set is a no-op — it keeps whatever schema it was first created with,
/// editing a migration has no effect locally, and the suite goes on testing a
/// table that no longer matches the file.
///
/// **`SYNC`, because the default defers.** An Atomic database's drop renames
/// its metadata aside and reclaims it `database_atomic_delay_before_drop_table_sec`
/// later — 480 seconds. One test per database and a suite re-run every few
/// minutes then accumulates faster than the server clears, and the server that
/// runs out of room is the developer's rather than CI's.
pub(super) async fn migrated_database(name: &str) -> Client {
    let bootstrap = build_client(config("default"));
    bootstrap
        .query(&format!("DROP DATABASE IF EXISTS {name} SYNC"))
        .execute()
        .await
        .unwrap();
    bootstrap
        .query(&format!("CREATE DATABASE {name}"))
        .execute()
        .await
        .unwrap();

    // The corpus `bins/migrate` applies, in its order — not a second reading of
    // the directories, which could agree with the deployment today and not
    // tomorrow. No runner: the database was created empty a line ago, so there
    // is nothing to skip and nothing to record.
    let client = build_client(config(name));
    for embedded in migrations::CLICKHOUSE
        .iter()
        .flat_map(|source| source.files)
    {
        client.query(embedded.sql).execute().await.unwrap();
    }
    client
}

/// A database of this test's own, and a store over it.
pub(super) async fn store(test: &str) -> (ClickHousePositionStore, Client) {
    let client = migrated_database(&format!("rust_positions_{test}")).await;
    (ClickHousePositionStore::new(client.clone()), client)
}

/// A dispatched range, as the processor leaves it.
///
/// Append only. The TypeScript's `index` reverts the range first, because a
/// re-dispatch has to cancel what the last one wrote — and here every test
/// reads a database created empty a moment earlier, so a retraction has
/// nothing to cancel and the ledger's other half stays with the crate that
/// will own it.
pub(super) async fn index(client: &Client, batch: &[Event]) {
    append(client, "spoke_events", batch).await;
}

/// A reserve that resolves all the way to a token, and a Hub asset with a
/// checkpoint — the state valuation needs before it can produce a number.
///
/// Asset 7 borrows 400,000 of the 1,000,000 supplied, so the index actually
/// accrues: the short-circuit would hold it at RAY if nothing were drawn.
pub(super) async fn list_reserve(client: &Client, events: &[Event]) {
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

/// One wallet holding all five.
pub(super) async fn seed_five_reserves(client: &Client) {
    let batch: Vec<_> = RESERVES
        .iter()
        .enumerate()
        .map(|(position, reserve_id)| {
            let log = u32::try_from(position).unwrap();
            supply(At::block(100).log(log), ALICE, reserve_id, "500")
        })
        .collect();
    index(client, &batch).await;
}

/// The same wallet and the same reserve id on two Spokes, which is two
/// positions rather than one.
pub(super) async fn seed_both_spokes(client: &Client) {
    index(
        client,
        &[
            supply(At::block(100), ALICE, "7", "500"),
            supply(At::block(100).log(1).on(SECOND_SPOKE), ALICE, "7", "900"),
        ],
    )
    .await;
}
