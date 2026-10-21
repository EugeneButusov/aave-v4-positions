//! The corpus's source: who is in the fold, and when it reaches.

use clickhouse_client::{Config, build_client, clickhouse};
use serde::Deserialize;

/// One wallet and every Spoke it has touched, so the Spoke filter is exercised
/// with addresses that match something.
pub(crate) struct Wallet {
    pub(crate) user: String,
    pub(crate) spokes: Vec<String>,

    /// How many rows the fold holds for this wallet. An upper bound on what a
    /// page can carry — the read view hides a position whose shares have
    /// netted to zero — and the safe side to be wrong on, because a page size
    /// counted in needlessly costs one request and one counted out loses a
    /// page boundary nothing else would walk.
    pub(crate) positions: u64,
}

pub(crate) struct Fold {
    pub(crate) wallets: Vec<Wallet>,

    /// The oldest `block_timestamp` in the fold, so a report says what slice
    /// of history it was replaying rather than leaving it to be remembered.
    pub(crate) earliest: u32,

    /// The newest. A legal `as_of` that is derived rather than chosen, so two
    /// runs pin the same instant.
    pub(crate) latest: u32,

    /// The instant wallets were selected from, derived from `latest` rather
    /// than from the clock so two runs over one fold pick the same wallets.
    /// Zero selected every wallet there is.
    pub(crate) since: u32,
}

#[derive(clickhouse::Row, Deserialize)]
struct Row {
    user: String,
    spoke: String,
    positions: u64,
}

/// # Errors
///
/// Whatever the driver says, which for an empty or unmigrated database is the
/// most useful thing this binary can report.
pub(crate) async fn read(
    config: Config,
    chain_id: u32,
    window: u32,
) -> Result<Fold, clickhouse::error::Error> {
    let client = build_client(config);

    let (earliest, latest) = client
        .query(
            "SELECT toUnixTimestamp(min(block_timestamp)), \
                    toUnixTimestamp(max(block_timestamp)) \
             FROM user_positions WHERE chain_id = {chainId:UInt32}",
        )
        .param("chainId", chain_id)
        .fetch_one::<(u32, u32)>()
        .await?;

    let since = since(latest, window);

    // Selected on recency, counted over everything: the route answers a
    // wallet's whole listing whatever brought it into the window, so a count
    // taken inside the window would miss the page boundaries the rest of it
    // has. A `since` of zero is below every row, so it selects them all —
    // passed as seconds and converted here, because a `DateTime` parameter
    // refuses to parse `0`.
    let rows = client
        .query(
            "SELECT user, spoke, count() AS positions FROM user_positions \
             WHERE chain_id = {chainId:UInt32} AND user IN ( \
                 SELECT user FROM user_positions \
                 WHERE chain_id = {chainId:UInt32} \
                   AND block_timestamp >= toDateTime({since:UInt32}) \
             ) \
             GROUP BY user, spoke ORDER BY user, spoke",
        )
        .param("chainId", chain_id)
        .param("since", since)
        .fetch_all::<Row>()
        .await?;

    Ok(Fold {
        wallets: group(rows),
        earliest,
        latest,
        since,
    })
}

/// The instant wallets are selected from.
///
/// Taken from the fold's newest row rather than from the clock, so two runs
/// over one fold pick the same wallets. A window reaching past the start of
/// the fold saturates at zero, which selects every wallet there is — the same
/// thing asking for no window at all does.
fn since(latest: u32, window: u32) -> u32 {
    if window == 0 {
        return 0;
    }

    window
        .checked_mul(86_400)
        .and_then(|seconds| latest.checked_sub(seconds))
        .unwrap_or_default()
}

/// The rows arrive ordered by user, so one pass closes each wallet.
fn group(rows: Vec<Row>) -> Vec<Wallet> {
    let mut wallets: Vec<Wallet> = Vec::new();

    for row in rows {
        match wallets.last_mut() {
            Some(wallet) if wallet.user == row.user => {
                wallet.spokes.push(row.spoke);
                wallet.positions += row.positions;
            }
            _ => wallets.push(Wallet {
                user: row.user,
                spokes: vec![row.spoke],
                positions: row.positions,
            }),
        }
    }

    wallets
}

#[cfg(test)]
mod tests {
    use clickhouse_client::{Config, build_client};

    use super::{Row, group, read, since};

    /// A database of this test's own, with the migrations `bins/migrate`
    /// applies. One per test: cargo runs them against one server and they all
    /// write the same table names.
    async fn migrated(name: &str) -> Config {
        let config = |database: &str| Config {
            url: std::env::var("CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".to_owned()),
            database: database.to_owned(),
            user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
            password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
        };

        let bootstrap = build_client(config("default"));
        for statement in [
            format!("DROP DATABASE IF EXISTS {name} SYNC"),
            format!("CREATE DATABASE {name}"),
        ] {
            bootstrap.query(&statement).execute().await.unwrap();
        }

        let scratch = config(name);
        let client = build_client(scratch.clone());
        for embedded in migrations::CLICKHOUSE
            .iter()
            .flat_map(|source| source.files)
        {
            client.query(embedded.sql).execute().await.unwrap();
        }

        scratch
    }

    /// Straight into the fold's target table rather than through the ledger
    /// and its views, because nothing here reads a share balance.
    ///
    /// `events` is set because the table is a `SummingMergeTree`, which
    /// discards a row whose summed columns are all zero — rows seeded with
    /// those columns left at their defaults never arrive.
    async fn fold_rows(config: &Config, rows: &[(&str, &str, u32, u32)]) {
        let client = build_client(config.clone());

        for (user, spoke, reserve_id, at) in rows {
            client
                .query(&format!(
                    "INSERT INTO user_positions \
                     (chain_id, user, spoke, reserve_id, block_timestamp, events) \
                     VALUES (1, '{user}', '{spoke}', {reserve_id}, toDateTime({at}), 1)"
                ))
                .execute()
                .await
                .unwrap();
        }
    }

    const LATEST: u32 = 1_780_000_000;
    const DAY: u32 = 86_400;

    /// The decision the window rests on: a wallet is *selected* on whether it
    /// was touched recently, and its positions are *counted* over the whole
    /// fold — because the route answers its whole listing whatever brought it
    /// into the window, so a count taken inside the window would miss the page
    /// boundaries the rest of it has.
    #[tokio::test]
    async fn selects_on_recency_and_counts_over_everything() {
        let config = migrated("replay_window").await;
        fold_rows(
            &config,
            &[
                // Touched inside the window, with two older rows behind it.
                ("0xaa", "0x01", 1, LATEST),
                ("0xaa", "0x01", 2, LATEST - 40 * DAY),
                ("0xaa", "0x02", 3, LATEST - 40 * DAY),
                // Nothing recent.
                ("0xbb", "0x01", 1, LATEST - 30 * DAY),
                ("0xbb", "0x01", 2, LATEST - 29 * DAY),
                // Inside the window, one position.
                ("0xcc", "0x01", 1, LATEST - 3 * DAY),
            ],
        )
        .await;

        let fold = read(config, 1, 7).await.unwrap();
        let selected: Vec<(&str, u64)> = fold
            .wallets
            .iter()
            .map(|wallet| (wallet.user.as_str(), wallet.positions))
            .collect();

        assert_eq!(selected, [("0xaa", 3), ("0xcc", 1)]);
        assert_eq!(fold.wallets[0].spokes, ["0x01", "0x02"]);
        assert_eq!(fold.latest, LATEST);
        assert_eq!(fold.earliest, LATEST - 40 * DAY);
        assert_eq!(fold.since, LATEST - 7 * DAY);
    }

    #[tokio::test]
    async fn no_window_takes_every_wallet_the_fold_has() {
        let config = migrated("replay_no_window").await;
        fold_rows(
            &config,
            &[
                ("0xaa", "0x01", 1, LATEST),
                ("0xbb", "0x01", 1, LATEST - 300 * DAY),
            ],
        )
        .await;

        let fold = read(config, 1, 0).await.unwrap();

        assert_eq!(fold.wallets.len(), 2);
        assert_eq!(fold.since, 0);
    }

    fn rows(pairs: &[(&str, &str, u64)]) -> Vec<Row> {
        pairs
            .iter()
            .map(|(user, spoke, positions)| Row {
                user: (*user).to_owned(),
                spoke: (*spoke).to_owned(),
                positions: *positions,
            })
            .collect()
    }

    #[test]
    fn takes_the_window_from_the_fold_rather_than_the_clock() {
        let latest = 1_780_000_000;

        assert_eq!(since(latest, 7), latest - 7 * 86_400);
        assert_eq!(since(latest, 1), latest - 86_400);
    }

    /// Both ways of asking for everything land on the same answer, and
    /// neither of them panics: zero is below every row, and a window longer
    /// than the fold saturates there rather than wrapping.
    #[test]
    fn a_window_past_the_start_of_the_fold_selects_every_wallet() {
        assert_eq!(since(1_780_000_000, 0), 0);
        assert_eq!(since(1_780_000_000, 100_000), 0);
        assert_eq!(since(1_780_000_000, u32::MAX), 0);
        assert_eq!(since(0, 7), 0);
    }

    #[test]
    fn collects_every_spoke_a_wallet_has_touched() {
        let wallets = group(rows(&[
            ("0xaa", "0x01", 3),
            ("0xaa", "0x02", 4),
            ("0xbb", "0x01", 1),
        ]));

        assert_eq!(wallets.len(), 2);
        assert_eq!(wallets[0].user, "0xaa");
        assert_eq!(wallets[0].spokes, ["0x01", "0x02"]);
        assert_eq!(wallets[0].positions, 7);
        assert_eq!(wallets[1].spokes, ["0x01"]);
        assert_eq!(wallets[1].positions, 1);
    }

    /// The grouping is one pass over an ordered read, so a wallet whose rows
    /// are split would be listed twice and replayed twice. Harmless, and worth
    /// knowing is the behaviour rather than a merge nobody wrote.
    #[test]
    fn an_unordered_read_splits_a_wallet() {
        let wallets = group(rows(&[
            ("0xaa", "0x01", 1),
            ("0xbb", "0x01", 1),
            ("0xaa", "0x02", 1),
        ]));

        assert_eq!(wallets.len(), 3);
    }

    #[test]
    fn an_empty_fold_is_no_wallets() {
        assert!(group(rows(&[])).is_empty());
    }
}
