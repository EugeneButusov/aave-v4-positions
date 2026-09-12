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
//! `API_GLOBAL_PREFIX` and the cursor secret arrive with the route, the `OTEL_*`
//! group with `telemetry` — and it cannot drift from what the process actually
//! honours.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use tracing::level_filters::LevelFilter;

use env::{Env, Invalid};

pub(crate) struct Config {
    pub(crate) level: LevelFilter,
    pub(crate) pretty: bool,
    pub(crate) host: IpAddr,
    pub(crate) port: u16,
    pub(crate) grace: Duration,
    pub(crate) clickhouse: clickhouse_client::Config,
    pub(crate) postgres_url: String,
}

impl Config {
    /// # Errors
    ///
    /// [`Invalid`], listing every variable that could not be read.
    pub(crate) fn from_env() -> Result<Self, Invalid> {
        Self::parse(&std::env::vars().collect())
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

    fn parse(pairs: &[(&str, &str)]) -> Result<Config, Invalid> {
        let vars = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();

        Config::parse(&vars)
    }

    #[test]
    fn an_empty_environment_is_the_local_defaults() {
        let config = parse(&[]).expect("defaults should stand alone");

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
    }

    #[test]
    fn reads_every_variable_it_declares() {
        let config = parse(&[
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
        ])
        .expect("every value is valid");

        assert_eq!(config.level, LevelFilter::DEBUG);
        assert!(config.pretty);
        assert_eq!(config.host, IpAddr::from([127, 0, 0, 1]));
        assert_eq!(config.port, 8080);
        assert_eq!(config.grace, Duration::from_secs(5));
        assert_eq!(config.clickhouse.database, "aave");
        assert_eq!(config.clickhouse.password, "hunter2");
    }

    #[test]
    fn an_empty_password_is_a_password_and_not_an_absence() {
        // A container started with CLICKHOUSE_SKIP_USER_SETUP has none, and
        // falling back to the default here would send `default` instead.
        let config = parse(&[("CLICKHOUSE_PASSWORD", "")]).expect("empty is legitimate");

        assert_eq!(config.clickhouse.password, "");
    }

    #[test]
    fn sends_both_database_urls_through_the_url_reader() {
        // Which is the mapping and not the reader: a typo in either boots a
        // process that reports itself degraded forever rather than saying what
        // is wrong, and `text` would have accepted both.
        let refused =
            |pairs: &[(&str, &str)]| parse(pairs).err().expect("expected a refusal").to_string();

        assert!(refused(&[("CLICKHOUSE_URL", "http//localhost:8123")]).contains("must be a URL"));
        assert!(refused(&[("POSTGRES_URL", "postgres.local")]).contains("must be a URL"));
    }
}
