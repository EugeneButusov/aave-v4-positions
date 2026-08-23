//! The environment this process reads, parsed once, before anything else.
//!
//! **Every variable here has a reader.** The TypeScript declares the whole
//! contract in one schema, including the `OTEL_*` group its own comment admits
//! is read elsewhere; here a field nobody uses is `dead_code`, which the
//! workspace denies. So the list grows with the code that needs it —
//! `API_GLOBAL_PREFIX` and the cursor secret arrive with the route, the `OTEL_*`
//! group with `telemetry` — and it cannot drift from what the process actually
//! honours.
//!
//! **Every error at once, not the first.** That is what `z.prettifyError` buys
//! the service this replaces: a deployment with three variables wrong learns all
//! three on the first boot instead of one per crash loop.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;
use std::time::Duration;

use tracing::level_filters::LevelFilter;

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
        let mut env = Env {
            vars,
            errors: Vec::new(),
        };

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

        if env.errors.is_empty() {
            Ok(config)
        } else {
            Err(Invalid(env.errors))
        }
    }
}

/// Everything wrong with the environment, in the order the variables are read.
#[derive(Debug)]
pub(crate) struct Invalid(Vec<String>);

impl fmt::Display for Invalid {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "invalid environment configuration:")?;
        for problem in &self.0 {
            write!(out, "\n  {problem}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Invalid {}

/// The seven `pino` accepts, so a deployment's existing value keeps working.
///
/// `fatal` and `silent` have no `tracing` counterpart of their own — the first
/// is an error and the second is the absence of a level — and mapping them here
/// is cheaper than a migration note nobody reads at three in the morning.
const LEVELS: [(&str, LevelFilter); 7] = [
    ("fatal", LevelFilter::ERROR),
    ("error", LevelFilter::ERROR),
    ("warn", LevelFilter::WARN),
    ("info", LevelFilter::INFO),
    ("debug", LevelFilter::DEBUG),
    ("trace", LevelFilter::TRACE),
    ("silent", LevelFilter::OFF),
];

/// The four spellings the TypeScript accepts for a boolean.
const FLAGS: [(&str, bool); 4] = [("true", true), ("1", true), ("false", false), ("0", false)];

struct Env<'a> {
    vars: &'a HashMap<String, String>,
    errors: Vec<String>,
}

impl<'a> Env<'a> {
    /// Borrowed from the map rather than from `self`, so a value can be read
    /// and then handed to a method that records a problem.
    fn raw(&self, key: &str) -> Option<&'a str> {
        // An empty value is a value: `CLICKHOUSE_PASSWORD=` means no password,
        // and treating it as absent would substitute a default nobody asked for.
        self.vars.get(key).map(String::as_str)
    }

    fn reject(&mut self, key: &str, reason: &str) {
        self.errors.push(format!("{key}: {reason}"));
    }

    fn text(&self, key: &str, default: &str) -> String {
        self.raw(key).unwrap_or(default).to_owned()
    }

    /// As lenient as `z.url()`, deliberately.
    ///
    /// Zod validates with `new URL()`, and the `url` crate implements the same
    /// WHATWG standard, so the two accept and reject exactly the same strings —
    /// `clickhouse:8123` included, which both read as a scheme and a path.
    /// Requiring `http` here would be a stricter boot contract than the service
    /// being replaced, and the driver is the authority on its own URL anyway:
    /// `build_pool` parses this again with libpq's rules.
    fn url(&mut self, key: &str, default: &str) -> String {
        let value = self.text(key, default);
        if url::Url::parse(&value).is_err() {
            self.reject(key, &format!("must be a URL, got {value:?}"));
        }
        value
    }

    fn address(&mut self, key: &str, default: &str) -> IpAddr {
        let value = self.text(key, default);
        value.parse().unwrap_or_else(|_| {
            // Stricter than `app.listen(port, host)`, which would resolve a
            // hostname. Every deployment of this sets an address, and a typo in
            // one should fail at boot rather than bind somewhere unintended.
            self.reject(key, &format!("must be an IP address, got {value:?}"));
            IpAddr::from([0, 0, 0, 0])
        })
    }

    fn port(&mut self, key: &str, default: u16) -> u16 {
        let Some(value) = self.raw(key) else {
            return default;
        };

        // `u16` is the range: 0 is "any port", which is never what a service
        // meant to be reachable at a known address was asking for.
        match value.parse::<u16>() {
            Ok(port) if port > 0 => port,
            _ => {
                self.reject(key, &format!("must be 1..=65535, got {value:?}"));
                default
            }
        }
    }

    fn seconds(&mut self, key: &str, default: u64, max: u64) -> u64 {
        let Some(value) = self.raw(key) else {
            return default;
        };

        match value.parse::<u64>() {
            Ok(seconds) if seconds <= max => seconds,
            _ => {
                self.reject(key, &format!("must be 0..={max}, got {value:?}"));
                default
            }
        }
    }

    fn flag(&mut self, key: &str, default: bool) -> bool {
        let Some(value) = self.raw(key) else {
            return default;
        };

        self.one_of(key, value, &FLAGS).unwrap_or(default)
    }

    fn level(&mut self, key: &str) -> LevelFilter {
        let Some(value) = self.raw(key) else {
            return LevelFilter::INFO;
        };

        self.one_of(key, value, &LEVELS)
            .unwrap_or(LevelFilter::INFO)
    }

    fn one_of<T: Copy>(&mut self, key: &str, value: &str, table: &[(&str, T)]) -> Option<T> {
        let found = table
            .iter()
            .find(|(spelling, _)| *spelling == value)
            .map(|(_, mapped)| *mapped);

        if found.is_none() {
            let allowed: Vec<_> = table.iter().map(|(spelling, _)| *spelling).collect();
            self.reject(
                key,
                &format!("must be one of {}, got {value:?}", allowed.join(", ")),
            );
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(pairs: &[(&str, &str)]) -> Result<Config, Invalid> {
        let vars = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();

        Config::parse(&vars)
    }

    fn problems(pairs: &[(&str, &str)]) -> Vec<String> {
        parse(pairs).err().expect("expected a refusal").0
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
    fn reports_every_bad_variable_rather_than_the_first() {
        // The property the collection exists for: one boot, one list, rather
        // than three restarts each naming the next mistake.
        let problems = problems(&[
            ("LOG_LEVEL", "loud"),
            ("API_PORT", "0"),
            ("SHUTDOWN_GRACE_SECONDS", "600"),
        ]);

        assert_eq!(problems.len(), 3);
        assert!(problems[0].starts_with("LOG_LEVEL: must be one of fatal, error, warn"));
        assert!(problems[1].starts_with("API_PORT: must be 1..=65535"));
        assert!(problems[2].starts_with("SHUTDOWN_GRACE_SECONDS: must be 0..=300"));
    }

    #[test]
    fn rejects_a_port_at_either_end_of_its_range() {
        assert!(problems(&[("API_PORT", "0")])[0].contains("got \"0\""));
        assert!(problems(&[("API_PORT", "65536")])[0].contains("got \"65536\""));
    }

    #[test]
    fn rejects_a_grace_window_past_the_ceiling_but_allows_none() {
        assert!(problems(&[("SHUTDOWN_GRACE_SECONDS", "301")])[0].contains("0..=300"));
        assert_eq!(
            parse(&[("SHUTDOWN_GRACE_SECONDS", "0")])
                .expect("zero is a choice")
                .grace,
            Duration::ZERO
        );
    }

    #[test]
    fn rejects_a_host_that_is_not_an_address() {
        assert!(problems(&[("API_HOST", "localhost")])[0].contains("must be an IP address"));
    }

    #[test]
    fn rejects_a_url_that_is_not_one() {
        // Both database URLs, because a typo in either boots a process that
        // reports itself degraded forever rather than saying what is wrong.
        // A missing scheme is what neither `new URL()` nor this will take.
        assert!(
            problems(&[("CLICKHOUSE_URL", "http//localhost:8123")])[0].contains("must be a URL")
        );
        assert!(problems(&[("POSTGRES_URL", "postgres.local")])[0].contains("must be a URL"));
    }

    #[test]
    fn takes_a_scheme_it_does_not_recognise() {
        // `new URL("clickhouse:8123")` is valid and so is this, and the point
        // is that the two agree rather than that the value is sensible.
        parse(&[("CLICKHOUSE_URL", "clickhouse:8123")]).expect("as lenient as z.url()");
    }

    #[test]
    fn maps_the_log_levels_pino_spells_differently() {
        // A deployment already setting either of these keeps working.
        assert_eq!(
            parse(&[("LOG_LEVEL", "fatal")]).expect("fatal maps").level,
            LevelFilter::ERROR
        );
        assert_eq!(
            parse(&[("LOG_LEVEL", "silent")])
                .expect("silent maps")
                .level,
            LevelFilter::OFF
        );
    }

    #[test]
    fn accepts_all_four_spellings_of_a_flag() {
        for (spelling, expected) in [("true", true), ("1", true), ("false", false), ("0", false)] {
            let config = parse(&[("LOG_PRETTY", spelling)]).expect("a known spelling");
            assert_eq!(config.pretty, expected, "LOG_PRETTY={spelling}");
        }
    }

    #[test]
    fn the_message_lists_the_problems_one_per_line() {
        let refusal = parse(&[("API_PORT", "0"), ("API_HOST", "nowhere")])
            .err()
            .expect("expected a refusal")
            .to_string();

        assert_eq!(
            refusal,
            "invalid environment configuration:\n  \
             API_HOST: must be an IP address, got \"nowhere\"\n  \
             API_PORT: must be 1..=65535, got \"0\""
        );
    }
}
