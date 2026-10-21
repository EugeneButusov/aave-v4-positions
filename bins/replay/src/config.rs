//! Where the two services are, and how much slack the clocks get.

use std::str::FromStr;

use env::{Env, Source};

pub(crate) struct Config {
    /// The service every answer is held against.
    pub(crate) oracle: String,
    pub(crate) candidate: String,
    pub(crate) chain_id: u32,

    /// What both deployments mount the versioned routes under. The probes are
    /// outside it on either service.
    pub(crate) prefix: String,

    /// How far apart two requests that cannot be issued at the same instant
    /// may leave a clock-derived number.
    pub(crate) skew: u64,

    /// How many wallets of the fold to replay. Zero is all of them.
    pub(crate) wallets: usize,
    pub(crate) clickhouse: clickhouse_client::Config,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Environment(#[from] env::Invalid),

    #[error("invalid environment configuration: {key}: must be a whole number, got {value:?}")]
    NotANumber { key: &'static str, value: String },
}

impl Config {
    /// # Errors
    ///
    /// [`Error`] naming the first variable that will not read.
    pub(crate) fn from_env() -> Result<Self, Error> {
        let vars = Source::from_env().read();

        Self::read(&Env::new(&vars))
    }

    /// # Errors
    ///
    /// [`Error`] naming the first variable that will not read.
    fn read(env: &Env<'_>) -> Result<Self, Error> {
        Ok(Self {
            oracle: env.url("REPLAY_ORACLE_URL", "http://localhost:3000")?,
            candidate: env.url("REPLAY_CANDIDATE_URL", "http://localhost:3002")?,
            chain_id: number(env, "REPLAY_CHAIN_ID", 1)?,
            prefix: env.text("REPLAY_PREFIX", "api"),
            skew: env.seconds("REPLAY_SKEW_SECONDS", 5, 3600)?,
            wallets: number(env, "REPLAY_WALLETS", 0)?,
            clickhouse: clickhouse_client::Config {
                url: env.url("CLICKHOUSE_URL", "http://localhost:8123")?,
                database: env.text("CLICKHOUSE_DATABASE", "aave"),
                user: env.text("CLICKHOUSE_USER", "default"),
                password: env.text("CLICKHOUSE_PASSWORD", ""),
            },
        })
    }
}

/// `env` reads ports, seconds and ratios; neither of these is one of those.
fn number<T: FromStr>(env: &Env<'_>, key: &'static str, default: T) -> Result<T, Error> {
    let value = env.text(key, "");

    if value.is_empty() {
        return Ok(default);
    }

    value.parse().map_err(|_| Error::NotANumber { key, value })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use env::Env;

    use super::{Config, number};

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn an_absent_number_is_the_default() {
        let vars = env(&[]);

        assert_eq!(
            number::<u32>(&Env::new(&vars), "REPLAY_CHAIN_ID", 1).unwrap(),
            1
        );
    }

    #[test]
    fn a_number_that_is_set_is_read() {
        let vars = env(&[("REPLAY_CHAIN_ID", "8453")]);

        assert_eq!(
            number::<u32>(&Env::new(&vars), "REPLAY_CHAIN_ID", 1).unwrap(),
            8453
        );
    }

    /// The defaults are the two compose publishes and the ClickHouse a
    /// developer already has.
    #[test]
    fn an_unconfigured_run_points_at_the_local_pair() {
        let vars = env(&[]);
        let config = Config::read(&Env::new(&vars)).unwrap();

        assert_eq!(config.oracle, "http://localhost:3000");
        assert_eq!(config.candidate, "http://localhost:3002");
        assert_eq!(config.chain_id, 1);
        assert_eq!(config.prefix, "api");
        assert_eq!(config.skew, 5);
        assert_eq!(config.wallets, 0);
        assert_eq!(config.clickhouse.database, "aave");
    }

    #[test]
    fn every_variable_reaches_the_run_it_configures() {
        let vars = env(&[
            ("REPLAY_ORACLE_URL", "http://node:3000"),
            ("REPLAY_CANDIDATE_URL", "http://rust:3000"),
            ("REPLAY_CHAIN_ID", "8453"),
            ("REPLAY_PREFIX", "gateway"),
            ("REPLAY_SKEW_SECONDS", "30"),
            ("REPLAY_WALLETS", "12"),
            ("CLICKHOUSE_URL", "http://clickhouse:8123"),
            ("CLICKHOUSE_DATABASE", "fold"),
            ("CLICKHOUSE_USER", "aave"),
            ("CLICKHOUSE_PASSWORD", "secret"),
        ]);
        let config = Config::read(&Env::new(&vars)).unwrap();

        assert_eq!(config.oracle, "http://node:3000");
        assert_eq!(config.candidate, "http://rust:3000");
        assert_eq!(config.chain_id, 8453);
        assert_eq!(config.prefix, "gateway");
        assert_eq!(config.skew, 30);
        assert_eq!(config.wallets, 12);
        assert_eq!(config.clickhouse.url, "http://clickhouse:8123");
        assert_eq!(config.clickhouse.database, "fold");
        assert_eq!(config.clickhouse.user, "aave");
        assert_eq!(config.clickhouse.password, "secret");
    }

    /// `"3000"` is the realistic typo and it is refused. `"localhost:3000"` is
    /// not: `url` reads `localhost` as the scheme, so that one reaches the
    /// socket and fails there instead — measured, not assumed.
    #[test]
    fn a_base_url_that_is_not_one_is_refused_at_the_start() {
        let lenient = env(&[("REPLAY_ORACLE_URL", "localhost:3000")]);
        assert!(Config::read(&Env::new(&lenient)).is_ok());

        let vars = env(&[("REPLAY_ORACLE_URL", "3000")]);

        // Not `unwrap_err`: that wants `Debug` on `Config`, and deriving it
        // would put the ClickHouse password in any panic message.
        assert_eq!(
            Config::read(&Env::new(&vars)).err().map(|refused| refused.to_string()),
            Some(
                r#"invalid environment configuration: REPLAY_ORACLE_URL: must be a URL, got "3000""#
                    .to_owned()
            )
        );
    }

    /// Refused rather than defaulted: a typo that silently replays chain 1 is
    /// a green run against the wrong fold.
    #[test]
    fn a_number_that_will_not_parse_is_refused() {
        let vars = env(&[("REPLAY_CHAIN_ID", "mainnet")]);

        assert_eq!(
            number::<u32>(&Env::new(&vars), "REPLAY_CHAIN_ID", 1)
                .unwrap_err()
                .to_string(),
            r#"invalid environment configuration: REPLAY_CHAIN_ID: must be a whole number, got "mainnet""#
        );
    }
}
