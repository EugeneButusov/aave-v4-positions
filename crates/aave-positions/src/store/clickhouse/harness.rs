//! A migrated database, a store over it, and the events put into it.
//!
//! Not fixtures — those are [`crate::store::fixtures`], and they are data. This
//! is what stands a scenario up.
//!
//! Seeded through the fold, not around it: these append decoded events and let
//! the materialized views do the work, so the specs are store specs rather
//! than assertions about a hand-written table.
//!
//! The write path is reproduced rather than imported, because the event ledgers
//! belong to a crate that does not exist yet.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use clickhouse_client::clickhouse::{self, Client};
use clickhouse_client::{Config, build_client};
use serde::Serialize;

use super::ClickHousePositionStore;
use crate::store::fixtures::{CHAIN_ID, Event};

/// One row of either ledger; both have the same shape.
#[derive(clickhouse::Row, Serialize)]
struct LedgerRow<'a> {
    chain_id: u64,
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

/// One stamp per batch — half of the ledger's pairing key, so a row needs one
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

pub(super) async fn append(client: &Client, table: &str, events: &[Event]) {
    if events.is_empty() {
        return;
    }

    let version = next_version();
    let mut insert = client.insert::<LedgerRow<'_>>(table).await.unwrap();
    for event in events {
        insert
            .write(&LedgerRow {
                chain_id: u64::from(CHAIN_ID),
                address: event.address.to_string().to_lowercase(),
                block_number: event.at.block,
                log_index: event.at.log,
                version,
                block_hash: "0xaaaa",
                block_timestamp: event.timestamp,
                tx_hash: "0xbbbb",
                tx_index: 0,
                event_name: event.name,
                // The projections read every field out of `body`.
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

fn config(database: &str) -> Config {
    Config {
        url: std::env::var("CLICKHOUSE_URL").unwrap_or_else(|_| "http://localhost:8123".to_owned()),
        transport_security: clickhouse_client::TransportSecurity::AllowInsecureHttp,
        database: database.to_owned(),
        user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
        password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
        max_execution: std::time::Duration::from_secs(60),
    }
}

/// A migrated database of the caller's own.
///
/// One per test, not one per file: `cargo` runs these concurrently against one
/// server and they all write the same table names.
///
/// Dropped first rather than `IF NOT EXISTS`, which every migration is written
/// with — so against a database an earlier run left behind the whole set is a
/// no-op and editing a migration has no local effect.
///
/// `SYNC`, because an Atomic database's drop otherwise reclaims its metadata
/// 480 seconds later, which a suite re-run every few minutes outpaces.
pub(super) async fn migrated_database(name: &str) -> Client {
    let bootstrap = build_client(config("default")).unwrap();
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

    // The corpus `bins/migrate` applies, in its order, rather than a second
    // reading of the directories. No runner: the database is empty.
    let client = build_client(config(name)).unwrap();
    for embedded in migrations::CLICKHOUSE
        .iter()
        .flat_map(|source| source.files)
    {
        client.query(embedded.sql).execute().await.unwrap();
    }
    client
}

pub(super) async fn store(test: &str) -> (ClickHousePositionStore, Client) {
    let client = migrated_database(&format!("rust_positions_{test}")).await;
    (ClickHousePositionStore::new(client.clone()), client)
}

/// A dispatched range, as the processor leaves it. Append only: every test
/// reads a database created empty, so a retraction has nothing to cancel.
pub(super) async fn index(client: &Client, batch: &[Event]) {
    append(client, "spoke_events", batch).await;
}
