//! The corpus's source: who is in the fold, and the latest instant in it.

use clickhouse_client::{Config, build_client, clickhouse};
use serde::Deserialize;

/// One wallet and every Spoke it has touched, so the Spoke filter is exercised
/// with addresses that match something.
pub(crate) struct Wallet {
    pub(crate) user: String,
    pub(crate) spokes: Vec<String>,
}

pub(crate) struct Fold {
    pub(crate) wallets: Vec<Wallet>,

    /// The newest `block_timestamp` in the fold. A legal `as_of` that is
    /// derived rather than chosen, so two runs pin the same instant.
    pub(crate) latest: u32,
}

#[derive(clickhouse::Row, Deserialize)]
struct Row {
    user: String,
    spoke: String,
}

/// # Errors
///
/// Whatever the driver says, which for an empty or unmigrated database is the
/// most useful thing this binary can report.
pub(crate) async fn read(config: Config, chain_id: u32) -> Result<Fold, clickhouse::error::Error> {
    let client = build_client(config);

    let rows = client
        .query(
            "SELECT user, spoke FROM user_positions \
             WHERE chain_id = {chainId:UInt32} \
             GROUP BY user, spoke ORDER BY user, spoke",
        )
        .param("chainId", chain_id)
        .fetch_all::<Row>()
        .await?;

    let latest = client
        .query(
            "SELECT toUnixTimestamp(max(block_timestamp)) FROM user_positions \
             WHERE chain_id = {chainId:UInt32}",
        )
        .param("chainId", chain_id)
        .fetch_one::<u32>()
        .await?;

    Ok(Fold {
        wallets: group(rows),
        latest,
    })
}

/// The rows arrive ordered by user, so one pass closes each wallet.
fn group(rows: Vec<Row>) -> Vec<Wallet> {
    let mut wallets: Vec<Wallet> = Vec::new();

    for row in rows {
        match wallets.last_mut() {
            Some(wallet) if wallet.user == row.user => wallet.spokes.push(row.spoke),
            _ => wallets.push(Wallet {
                user: row.user,
                spokes: vec![row.spoke],
            }),
        }
    }

    wallets
}

#[cfg(test)]
mod tests {
    use super::{Row, group};

    fn rows(pairs: &[(&str, &str)]) -> Vec<Row> {
        pairs
            .iter()
            .map(|(user, spoke)| Row {
                user: (*user).to_owned(),
                spoke: (*spoke).to_owned(),
            })
            .collect()
    }

    #[test]
    fn collects_every_spoke_a_wallet_has_touched() {
        let wallets = group(rows(&[
            ("0xaa", "0x01"),
            ("0xaa", "0x02"),
            ("0xbb", "0x01"),
        ]));

        assert_eq!(wallets.len(), 2);
        assert_eq!(wallets[0].user, "0xaa");
        assert_eq!(wallets[0].spokes, ["0x01", "0x02"]);
        assert_eq!(wallets[1].spokes, ["0x01"]);
    }

    /// The grouping is one pass over an ordered read, so a wallet whose rows
    /// are split would be listed twice and replayed twice. Harmless, and worth
    /// knowing is the behaviour rather than a merge nobody wrote.
    #[test]
    fn an_unordered_read_splits_a_wallet() {
        let wallets = group(rows(&[
            ("0xaa", "0x01"),
            ("0xbb", "0x01"),
            ("0xaa", "0x02"),
        ]));

        assert_eq!(wallets.len(), 3);
    }

    #[test]
    fn an_empty_fold_is_no_wallets() {
        assert!(group(rows(&[])).is_empty());
    }
}
