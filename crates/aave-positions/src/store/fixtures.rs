//! A migrated database, and the ledgers the store reads.
//!
//! **Seeded through the fold, not around it.** These append decoded events and
//! let the materialized views do the work, which is what makes the tests below
//! store specs rather than assertions about a hand-written table. Writing into
//! `user_positions` directly would prove the SQL in this module and nothing
//! about the SQL it reads.
//!
//! The write path is reproduced here rather than imported: the event ledgers
//! belong to a crate that does not exist yet. It is two statements — an insert,
//! and a sign-flipped copy — and both are what the ledger's own migration
//! documents.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use alloy_primitives::{Address, address};
use clickhouse_client::clickhouse::{self, Client};
use clickhouse_client::{Config, build_client};
use serde::Serialize;

use super::{ClickHousePositionStore, PositionPage, PositionQuery};

pub(super) const CHAIN_ID: u32 = 1;
pub(super) const SPOKE: Address = address!("0x94e7a5dcbe816e498b89ab752661904e2f56c485");
/// The Bluechip Spoke — a second isolated margin account for the same wallet.
/// Sorts *after* [`SPOKE`] as a string (`0x94…` < `0x97…`), which is what makes
/// a cross-Spoke page order assertable rather than incidental.
pub(super) const SECOND_SPOKE: Address = address!("0x973a023a77420ba610f06b3858ad991df6d85a08");
/// Checksummed, as viem hands them back. Lower-casing them is the fold's job.
pub(super) const ALICE: Address = address!("0x82D16fF1C724ab72F218A3f7f6DD3E5385ee87E8");
pub(super) const BOB: Address = address!("0xB8516f75DCf450b5b455b5114F5a92F6abD37dCa");
/// A position manager, and never the owner of a position (§2).
pub(super) const ROUTER: Address = address!("0xe68ab4F90Fe026B9873F5F276eD2d7efBbbE42Be");
/// Past 2^53 and past Int64 max, which is where `JSONExtractInt` returns 0.
pub(super) const HUGE: &str = "422166581625087607993";

/// Core Hub on mainnet, lower-cased as a log's address arrives.
pub(super) const HUB: Address = address!("0xcca852bc40e560adc3b1cc58ca5b55638ce826c9");
/// USDC, and the `underlying` the `AddAsset` builder reports.
pub(super) const USDC: Address = address!("0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
/// RAY, where `drawnIndex` starts and the floor it never goes below.
pub(super) const RAY: &str = "1000000000000000000000000000";

const EVENT_MIGRATIONS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/aave-positions/events/src/store/clickhouse-migrations"
);
const POSITION_MIGRATIONS: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../packages/aave-positions/positions/src/store/clickhouse-migrations"
);

/// Where a log sat. `spoke` defaults to [`SPOKE`].
#[derive(Debug, Clone, Copy)]
pub(super) struct At {
    pub block: u64,
    pub log: u32,
    pub spoke: Address,
}

impl At {
    pub(super) fn block(block: u64) -> Self {
        Self {
            block,
            log: 0,
            spoke: SPOKE,
        }
    }

    pub(super) fn log(self, log: u32) -> Self {
        Self { log, ..self }
    }

    pub(super) fn on(self, spoke: Address) -> Self {
        Self { spoke, ..self }
    }
}

/// A decoded log, shaped as the decoder would hand it over.
#[derive(Debug, Clone)]
pub(super) struct Event {
    at: At,
    address: Address,
    /// Distinct per block on the Hub side, so `index_timestamp` can be checked
    /// against the checkpoint it belongs to rather than a constant.
    timestamp: u32,
    name: &'static str,
    body: String,
}

/// The base instant every fixture below counts from.
pub(super) const T0: u64 = 1_785_000_000;

fn spoke_event(name: &'static str, at: At, body: String) -> Event {
    Event {
        at,
        address: at.spoke,
        timestamp: 1_785_000_000,
        name,
        body,
    }
}

fn hub_event(name: &'static str, at: At, body: String) -> Event {
    Event {
        at,
        address: HUB,
        timestamp: u32::try_from(T0.saturating_add(at.block)).unwrap_or(u32::MAX),
        name,
        body,
    }
}

// --- the Spoke ledger -------------------------------------------------------

pub(super) fn supply(at: At, user: Address, reserve_id: &str, shares: &str) -> Event {
    supplied_by(at, user, user, reserve_id, shares)
}

/// A supply the caller routed for someone else — the §2 case.
pub(super) fn supplied_by(
    at: At,
    caller: Address,
    user: Address,
    reserve_id: &str,
    shares: &str,
) -> Event {
    spoke_event(
        "Supply",
        at,
        format!(
            r#"{{"reserveId":"{reserve_id}","caller":"{caller}","user":"{user}","suppliedShares":"{shares}","suppliedAmount":"{shares}"}}"#
        ),
    )
}

pub(super) fn withdraw(at: At, user: Address, reserve_id: &str, shares: &str) -> Event {
    spoke_event(
        "Withdraw",
        at,
        format!(
            r#"{{"reserveId":"{reserve_id}","caller":"{user}","user":"{user}","withdrawnShares":"{shares}","withdrawnAmount":"{shares}"}}"#
        ),
    )
}

pub(super) fn borrow(at: At, user: Address, reserve_id: &str, shares: &str) -> Event {
    spoke_event(
        "Borrow",
        at,
        format!(
            r#"{{"reserveId":"{reserve_id}","caller":"{user}","user":"{user}","drawnShares":"{shares}","drawnAmount":"{shares}"}}"#
        ),
    )
}

pub(super) fn add_reserve(at: At, reserve_id: &str, asset_id: &str, hub: Address) -> Event {
    spoke_event(
        "AddReserve",
        at,
        format!(r#"{{"reserveId":"{reserve_id}","assetId":"{asset_id}","hub":"{hub}"}}"#),
    )
}

// --- the Hub ledger ---------------------------------------------------------

/// The asset every Hub fixture below moves.
const ASSET: &str = "7";

pub(super) fn add_asset(at: At, underlying: Address, decimals: u8) -> Event {
    hub_event(
        "AddAsset",
        at,
        format!(r#"{{"assetId":"{ASSET}","underlying":"{underlying}","decimals":{decimals}}}"#),
    )
}

pub(super) fn add(at: At, shares: &str, amount: &str) -> Event {
    hub_event(
        "Add",
        at,
        format!(
            r#"{{"assetId":"{ASSET}","spoke":"{SPOKE}","shares":"{shares}","amount":"{amount}"}}"#
        ),
    )
}

pub(super) fn draw(at: At, drawn_shares: &str, drawn_amount: &str) -> Event {
    hub_event(
        "Draw",
        at,
        format!(
            r#"{{"assetId":"{ASSET}","spoke":"{SPOKE}","drawnShares":"{drawn_shares}","drawnAmount":"{drawn_amount}"}}"#
        ),
    )
}

pub(super) fn update_asset(at: At, drawn_index: &str, drawn_rate: &str) -> Event {
    hub_event(
        "UpdateAsset",
        at,
        format!(
            r#"{{"assetId":"{ASSET}","drawnIndex":"{drawn_index}","drawnRate":"{drawn_rate}","accruedFees":"0"}}"#
        ),
    )
}

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

/// One stamp per batch, monotonic rather than the wall clock.
///
/// It only has to differ between successive states of the same log. The
/// TypeScript uses `Date.now()`, which is safe there because two dispatches of
/// one range are separated by at least an RPC round trip — and is not safe
/// here, where a test reverts and re-appends inside the same millisecond and
/// the engine would then see the replacement as a third row of the pair.
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

/// Copies the live rows of a range back in with the sign flipped.
///
/// Server-side `INSERT … SELECT` rather than reading rows out and writing them
/// back: the retraction has to reproduce every column and the original version
/// for the engine to pair it, and selecting from the view is the only way to be
/// sure it does.
pub(super) async fn revert(client: &Client, table: &str, from: u64, to: u64) {
    const COLUMNS: &str = "chain_id, address, block_number, log_index, block_hash, \
         block_timestamp, tx_hash, tx_index, event_name, topic1, topic2, topic3, body, data";

    client
        .query(&format!(
            "INSERT INTO {table} ({COLUMNS}, version, sign)
             SELECT {COLUMNS}, version, -1
             FROM {table}_current
             WHERE chain_id = {{chainId:UInt32}}
               AND block_number BETWEEN {{fromBlock:UInt64}} AND {{toBlock:UInt64}}"
        ))
        .param("chainId", CHAIN_ID)
        .param("fromBlock", from)
        .param("toBlock", to)
        .execute()
        .await
        .unwrap();
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

    let client = build_client(config(name));
    for file in migration_files() {
        client
            .query(&std::fs::read_to_string(&file).unwrap())
            .execute()
            .await
            .unwrap();
    }
    client
}

/// Every `.sql` in both directories, ordered by filename **across** them.
///
/// Which is what puts the projections after the table they read: `010` beats
/// `002` wherever each of them lives. Read at run time rather than embedded,
/// because a second copy of the list `bins/migrate` owns is a second thing to
/// forget — and a test may read the filesystem where a shipped binary may not.
///
/// Not a migration runner. `bins/migrate` is the only one and the only thing
/// that keeps a ledger; the database above was created empty a line ago, so
/// there is nothing to skip and nothing to record.
fn migration_files() -> Vec<std::path::PathBuf> {
    let mut files: Vec<_> = [EVENT_MIGRATIONS, POSITION_MIGRATIONS]
        .iter()
        .flat_map(|directory| std::fs::read_dir(directory).unwrap())
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "sql"))
        .collect();

    files.sort_by_key(|path| path.file_name().map(std::ffi::OsString::from));
    files
}

// --- what every spec starts from -------------------------------------------

/// The block every Hub fixture checkpoints at, and the instant it lands on.
const CHECKPOINT_BLOCK: u64 = 100;
pub(super) const CHECKPOINT_AT: u64 = T0 + CHECKPOINT_BLOCK;
pub(super) const YEAR: u64 = 365 * 24 * 3600;
/// 5% per annum, RAY-scaled, as `drawnRate` arrives on `UpdateAsset`.
pub(super) const FIVE_PERCENT: &str = "50000000000000000000000000";

/// A database of this test's own, and a store over it.
pub(super) async fn store(test: &str) -> (ClickHousePositionStore, Client) {
    let client = migrated_database(&format!("rust_positions_{test}")).await;
    (ClickHousePositionStore::new(client.clone()), client)
}

/// The query every case varies from: this wallet, on this Spoke, now.
pub(super) fn ask() -> PositionQuery {
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
pub(super) async fn index(client: &Client, from: u64, to: u64, batch: &[Event]) {
    revert(client, "spoke_events", from, to).await;
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

pub(super) fn reserve_ids(page: &PositionPage) -> Vec<String> {
    page.items
        .iter()
        .map(|item| item.reserve_id.to_string())
        .collect()
}

/// Five reserves whose ids are deliberately out of numeric order as text: 13
/// sorts before 3.
pub(super) const RESERVES: [&str; 5] = ["3", "7", "13", "21", "34"];

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
    index(client, 100, 100, &batch).await;
}

/// The same wallet and the same reserve id on two Spokes, which is two
/// positions rather than one.
pub(super) async fn seed_both_spokes(client: &Client) {
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
