//! The wallets, the events and the query every spec starts from.
//!
//! Nothing here writes anything anywhere: these are Aave's shapes — a decoded
//! log, the addresses it names, the page key a caller asks with — and they read
//! the same whichever store is under test. What puts them into a database is
//! [`super::clickhouse::fixtures`].

use alloy_primitives::{Address, address};

use super::{PositionPage, PositionQuery};

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
    pub at: At,
    pub address: Address,
    /// Distinct per block on the Hub side, so `index_timestamp` can be checked
    /// against the checkpoint it belongs to rather than a constant.
    pub timestamp: u32,
    pub name: &'static str,
    pub body: String,
}

/// The base instant every fixture below counts from.
pub(super) const T0: u64 = 1_785_000_000;

pub(super) fn spoke_event(name: &'static str, at: At, body: String) -> Event {
    Event {
        at,
        address: at.spoke,
        timestamp: 1_785_000_000,
        name,
        body,
    }
}

pub(super) fn hub_event(name: &'static str, at: At, body: String) -> Event {
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

/// The block every Hub fixture checkpoints at, and the instant it lands on.
pub(super) const CHECKPOINT_BLOCK: u64 = 100;
pub(super) const CHECKPOINT_AT: u64 = T0 + CHECKPOINT_BLOCK;
pub(super) const YEAR: u64 = 365 * 24 * 3600;
/// 5% per annum, RAY-scaled, as `drawnRate` arrives on `UpdateAsset`.
pub(super) const FIVE_PERCENT: &str = "50000000000000000000000000";

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

pub(super) fn reserve_ids(page: &PositionPage) -> Vec<String> {
    page.items
        .iter()
        .map(|item| item.reserve_id.to_string())
        .collect()
}

/// Five reserves whose ids are deliberately out of numeric order as text: 13
/// sorts before 3.
pub(super) const RESERVES: [&str; 5] = ["3", "7", "13", "21", "34"];
