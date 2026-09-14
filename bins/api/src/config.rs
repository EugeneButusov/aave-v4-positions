//! What this process is set up to be, parsed once, before anything else.
//!
//! The list below is the whole of it: every variable this service honours, the
//! reader it goes through and the default it falls back to. Reading and
//! validating are [`env`](mod@env)'s job — a port's range and a level's spelling mean
//! nothing here. This is the mapping from a container's
//! environment to a running service, and it is the only place that mapping
//! exists.
//!
//! **Every variable here has a reader.** The TypeScript declares the whole
//! contract in one schema, including the `OTEL_*` group its own comment admits
//! is read elsewhere; here a field nobody uses is `dead_code`, which the
//! workspace denies. So the list grows with the code that needs it —
//! `API_GLOBAL_PREFIX` and the cursor secret arrived with the route, the
//! `OTEL_*` group arrives with `telemetry` — and it cannot drift from what the
//! process actually honours.
//!
//! **One variable has no default.** A cursor signing key every deployment shares
//! is not a signature, so an unset `POSITIONS_CURSOR_SECRET` is a process that
//! refuses to boot rather than one that serves forgeable cursors.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use tracing::level_filters::LevelFilter;

use env::{Env, Invalid, Source};

use crate::positions::MIN_SECRET_BYTES;

/// A threshold past a day is one that never fires on a chain with twelve-second
/// blocks, so bounding it costs nothing and an unbounded one is a number with
/// no wrong value.
///
/// **A bound, not a units check.** It catches `300000` — the price default in
/// milliseconds — and not `60000`, which is the sync default in milliseconds,
/// under a day, and just as wrong. Reading a threshold in the wrong unit is what
/// `API_*_STALE_AFTER_SECONDS` is named to prevent; this only stops the value
/// being absurd.
const MAX_STALENESS_SECONDS: u64 = 86_400;

pub(crate) struct Config {
    pub(crate) level: LevelFilter,
    pub(crate) pretty: bool,
    pub(crate) host: IpAddr,
    pub(crate) port: u16,
    pub(crate) grace: Duration,
    /// What the versioned routes are mounted under. The probes are not: an
    /// orchestrator's check is not part of the API's versioned surface, and
    /// every compose healthcheck already asks for `/health/ready`.
    pub(crate) prefix: String,
    pub(crate) cursor_secret: String,
    pub(crate) staleness: Staleness,
    pub(crate) clickhouse: clickhouse_client::Config,
    pub(crate) postgres_url: String,
}

/// How old a number may be before a page says so.
///
/// **Two thresholds rather than one**, because the two clocks run at different
/// rates: the indexer advances every block, the oracle is read every minute. And
/// neither is the indexer's own `INDEXER_STALL_THRESHOLD_MS` — that one decides
/// whether to drain traffic from a pod; these tell a reader their numbers are a
/// minute old, which a reader wants to know long before an operator does.
///
/// Zero is a choice, and it means every page says so.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Staleness {
    pub(crate) sync: u64,

    /// **It measures how long since we last read the oracle, never how long
    /// since a feed last moved.** That distinction is the trap §7.5 names: an
    /// hour without an `AnswerUpdated` is ordinary Chainlink behaviour, so a
    /// threshold derived from feed cadence would mark healthy feeds stale
    /// forever and teach readers to ignore the flag.
    pub(crate) price: u64,
}

impl Config {
    /// Where the variables come from is [`Source`]'s to decide, and it decides
    /// from the environment it is about to read — so a deployment changes it by
    /// setting `APP_ENV` rather than by the binary being built differently.
    ///
    /// # Errors
    ///
    /// [`Invalid`], listing every variable that could not be read.
    pub(crate) fn from_env() -> Result<Self, Invalid> {
        Self::parse(&Source::from_env().read())
    }

    /// Takes the environment as a map rather than reading it, so a case can
    /// name three bad variables without touching global state the other tests
    /// are running against.
    fn parse(vars: &HashMap<String, String>) -> Result<Self, Invalid> {
        let mut env = Env::new(vars);

        let config = Self {
            level: env.level("LOG_LEVEL"),
            pretty: env.flag("LOG_PRETTY", false),
            host: env.address("API_HOST", "0.0.0.0"),
            port: env.port("API_PORT", 3000),
            grace: Duration::from_secs(env.seconds("SHUTDOWN_GRACE_SECONDS", 10, 300)),
            prefix: env.text("API_GLOBAL_PREFIX", "api"),
            cursor_secret: env.secret("POSITIONS_CURSOR_SECRET", MIN_SECRET_BYTES),
            staleness: Staleness {
                sync: env.seconds("API_SYNC_STALE_AFTER_SECONDS", 60, MAX_STALENESS_SECONDS),
                price: env.seconds("API_PRICE_STALE_AFTER_SECONDS", 300, MAX_STALENESS_SECONDS),
            },
            clickhouse: clickhouse_client::Config {
                url: env.url("CLICKHOUSE_URL", "http://localhost:8123"),
                database: env.text("CLICKHOUSE_DATABASE", "default"),
                user: env.text("CLICKHOUSE_USER", "default"),
                // Empty is legitimate: a container started with
                // `CLICKHOUSE_SKIP_USER_SETUP` has no password, which is how the
                // test and CI instances run.
                password: env.text("CLICKHOUSE_PASSWORD", ""),
            },
            postgres_url: env.url(
                "POSTGRES_URL",
                "postgres://postgres@localhost:5432/postgres",
            ),
        };

        env.finish()?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    //! The mapping, not the readers: which variable reaches which field, and
    //! what a deployment that sets none of them gets. [`env`](mod@env) proves that a
    //! port is a port.

    use super::*;

    /// Thirty-two bytes, which is what the cursor codec asks for.
    const SECRET: (&str, &str) = (
        "POSITIONS_CURSOR_SECRET",
        "a-test-key-that-is-long-enough!!",
    );

    fn parse(pairs: &[(&str, &str)]) -> Result<Config, Invalid> {
        let vars = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();

        Config::parse(&vars)
    }

    /// Every case that is not about the secret still has to set it: it is the
    /// one variable here with no default, and without it nothing else parses.
    fn configured(pairs: &[(&str, &str)]) -> Result<Config, Invalid> {
        let mut all = pairs.to_vec();
        all.push(SECRET);

        parse(&all)
    }

    #[test]
    fn an_empty_environment_is_the_local_defaults() {
        let config = configured(&[]).expect("defaults should stand alone");

        assert_eq!(config.level, LevelFilter::INFO);
        assert!(!config.pretty);
        assert_eq!(config.host, IpAddr::from([0, 0, 0, 0]));
        assert_eq!(config.port, 3000);
        assert_eq!(config.grace, Duration::from_secs(10));
        assert_eq!(config.clickhouse.url, "http://localhost:8123");
        assert_eq!(config.clickhouse.database, "default");
        assert_eq!(config.clickhouse.user, "default");
        assert_eq!(config.clickhouse.password, "");
        assert_eq!(
            config.postgres_url,
            "postgres://postgres@localhost:5432/postgres"
        );
        assert_eq!(config.prefix, "api");
        assert_eq!(config.staleness.sync, 60);
        assert_eq!(config.staleness.price, 300);
    }

    #[test]
    fn refuses_to_boot_without_a_cursor_secret() {
        // A default here would be a key every deployment shares, and a shared
        // key is not a signature. The alternative to this refusal is a process
        // that runs and serves forgeable cursors.
        let refusal = parse(&[]).err().expect("expected a refusal").to_string();

        assert!(refusal.contains("POSITIONS_CURSOR_SECRET"), "{refusal}");
        assert!(!refusal.contains("a-test-key"), "the key reached the log");
    }

    #[test]
    fn bounds_a_staleness_threshold_without_pretending_to_check_its_units() {
        // Both halves, because the bound only looks like a units check. The
        // price default in milliseconds is past a day and refused; the sync
        // default in milliseconds is under one and taken, which is why the name
        // carries the unit rather than the ceiling.
        let refusal = configured(&[("API_PRICE_STALE_AFTER_SECONDS", "300000")])
            .err()
            .expect("expected a refusal")
            .to_string();

        assert!(refusal.contains("must be 0..=86400"), "{refusal}");

        let taken = configured(&[("API_SYNC_STALE_AFTER_SECONDS", "60000")])
            .expect("under a day, and still wrong");

        assert_eq!(taken.staleness.sync, 60_000);
    }

    #[test]
    fn reads_every_variable_it_declares() {
        let config = configured(&[
            ("LOG_LEVEL", "debug"),
            ("LOG_PRETTY", "1"),
            ("API_HOST", "127.0.0.1"),
            ("API_PORT", "8080"),
            ("SHUTDOWN_GRACE_SECONDS", "5"),
            ("CLICKHOUSE_URL", "http://clickhouse:8123"),
            ("CLICKHOUSE_DATABASE", "aave"),
            ("CLICKHOUSE_USER", "aave"),
            ("CLICKHOUSE_PASSWORD", "hunter2"),
            ("POSTGRES_URL", "postgres://aave@postgres:5432/aave"),
            ("API_GLOBAL_PREFIX", "gateway"),
            ("API_SYNC_STALE_AFTER_SECONDS", "30"),
            ("API_PRICE_STALE_AFTER_SECONDS", "900"),
        ])
        .expect("every value is valid");

        assert_eq!(config.level, LevelFilter::DEBUG);
        assert!(config.pretty);
        assert_eq!(config.host, IpAddr::from([127, 0, 0, 1]));
        assert_eq!(config.port, 8080);
        assert_eq!(config.grace, Duration::from_secs(5));
        assert_eq!(config.clickhouse.database, "aave");
        assert_eq!(config.clickhouse.password, "hunter2");
        assert_eq!(config.prefix, "gateway");
        assert_eq!(config.cursor_secret, SECRET.1);
        assert_eq!(config.staleness.sync, 30);
        assert_eq!(config.staleness.price, 900);
    }

    #[test]
    fn an_empty_password_is_a_password_and_not_an_absence() {
        // A container started with CLICKHOUSE_SKIP_USER_SETUP has none, and
        // falling back to the default here would send `default` instead.
        let config = configured(&[("CLICKHOUSE_PASSWORD", "")]).expect("empty is legitimate");

        assert_eq!(config.clickhouse.password, "");
    }

    #[test]
    fn sends_both_database_urls_through_the_url_reader() {
        // Which is the mapping and not the reader: a typo in either boots a
        // process that reports itself degraded forever rather than saying what
        // is wrong, and `text` would have accepted both.
        let refused = |pairs: &[(&str, &str)]| {
            configured(pairs)
                .err()
                .expect("expected a refusal")
                .to_string()
        };

        assert!(refused(&[("CLICKHOUSE_URL", "http//localhost:8123")]).contains("must be a URL"));
        assert!(refused(&[("POSTGRES_URL", "postgres.local")]).contains("must be a URL"));
    }
}
