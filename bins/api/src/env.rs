//! Reading the container's contract: one variable at a time, collecting the
//! problems rather than stopping at the first.
//!
//! Below [`config`](crate::config) rather than inside it, and that boundary is
//! the point: nothing here knows what this service is. It knows that a port is
//! `1..=65535` and that `pino` spells a level seven ways. What those values
//! *mean* to a running process — which one the listener binds, which one the
//! shutdown waits out — is the layer above. crates.io draws the same line
//! harder, with `crates_io_env_vars` a crate of its own that its `config`
//! modules call into.
//!
//! **Every error at once, not the first.** That is what `z.prettifyError` buys
//! the service this replaces: a deployment with three variables wrong learns all
//! three on the first boot instead of one per crash loop. Every reader below
//! therefore returns a usable value *and* records a problem, so parsing
//! continues past a bad variable and [`Env::finish`] reports the lot.
//!
//! **Which is why this is hand-rolled, and the reason is measured.** The two
//! candidates were `clap`'s derive with `env =` on each field — what
//! `meilisearch` and `influxdb3` do, and by volume the ecosystem's default — and
//! `figment`, whose `Error` documents itself as possibly holding more than one.
//! Given `API_PORT=0`, `API_HOST=nowhere` and `SHUTDOWN_GRACE_SECONDS=600`, both
//! report exactly one: `figment`'s `count()` is 1 because serde stops at the
//! first bad field, and `clap` exits on the first it reaches. Neither can say
//! all three, so neither preserves the behaviour under a migration whose whole
//! rule is not changing behaviour.
//!
//! The shape is [linkerd2-proxy's](https://github.com/linkerd/linkerd2-proxy/blob/main/linkerd/app/src/env.rs),
//! arrived at independently and for the same reason — its `parse_config` says
//! "parse all the environment variables … defer returning any errors until all
//! of them have been parsed", and it takes its input through a trait so a test
//! can hand it a map instead of the process environment. It spends 1090 lines on
//! that; this spends a tenth of it on ten variables.
//!
//! `clap` still arrives with Phase 4's five CLIs, where the argument parsing is
//! the point. This binary takes no arguments.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;

use tracing::level_filters::LevelFilter;

/// The environment as a map, and what has been wrong with it so far.
pub(crate) struct Env<'a> {
    vars: &'a HashMap<String, String>,
    errors: Vec<String>,
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

impl<'a> Env<'a> {
    /// Takes the environment as a map rather than reading it, so a case can
    /// name three bad variables without touching global state the other tests
    /// are running against.
    pub(crate) fn new(vars: &'a HashMap<String, String>) -> Self {
        Self {
            vars,
            errors: Vec::new(),
        }
    }

    /// # Errors
    ///
    /// [`Invalid`], listing every variable that could not be read.
    pub(crate) fn finish(self) -> Result<(), Invalid> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(Invalid(self.errors))
        }
    }

    pub(crate) fn text(&self, key: &str, default: &str) -> String {
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
    pub(crate) fn url(&mut self, key: &str, default: &str) -> String {
        let value = self.text(key, default);
        if url::Url::parse(&value).is_err() {
            self.reject(key, &format!("must be a URL, got {value:?}"));
        }
        value
    }

    pub(crate) fn address(&mut self, key: &str, default: &str) -> IpAddr {
        let value = self.text(key, default);
        value.parse().unwrap_or_else(|_| {
            // Stricter than `app.listen(port, host)`, which would resolve a
            // hostname. Every deployment of this sets an address, and a typo in
            // one should fail at boot rather than bind somewhere unintended.
            self.reject(key, &format!("must be an IP address, got {value:?}"));
            IpAddr::from([0, 0, 0, 0])
        })
    }

    pub(crate) fn port(&mut self, key: &str, default: u16) -> u16 {
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

    pub(crate) fn seconds(&mut self, key: &str, default: u64, max: u64) -> u64 {
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

    pub(crate) fn flag(&mut self, key: &str, default: bool) -> bool {
        let Some(value) = self.raw(key) else {
            return default;
        };

        self.one_of(key, value, &FLAGS).unwrap_or(default)
    }

    pub(crate) fn level(&mut self, key: &str) -> LevelFilter {
        let Some(value) = self.raw(key) else {
            return LevelFilter::INFO;
        };

        self.one_of(key, value, &LEVELS)
            .unwrap_or(LevelFilter::INFO)
    }

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
    //! The readers on their own terms, with keys that mean nothing to this
    //! service. What the variables are actually called, and which reader each
    //! one gets, is [`config`](crate::config)'s to prove.

    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    /// Reads one key with `read`, and returns the value beside any problem.
    fn read<T>(pairs: &[(&str, &str)], read: impl FnOnce(&mut Env<'_>) -> T) -> (T, Vec<String>) {
        let map = vars(pairs);
        let mut env = Env::new(&map);
        let value = read(&mut env);

        (value, env.finish().err().map_or_else(Vec::new, |bad| bad.0))
    }

    fn problems<T>(pairs: &[(&str, &str)], reader: impl FnOnce(&mut Env<'_>) -> T) -> Vec<String> {
        read(pairs, reader).1
    }

    #[test]
    fn an_absent_variable_is_its_default_and_not_a_problem() {
        let (port, problems) = read(&[], |env| env.port("PORT", 3000));

        assert_eq!(port, 3000);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn an_empty_value_is_a_value_and_not_an_absence() {
        // A container started with CLICKHOUSE_SKIP_USER_SETUP has no password,
        // and falling back to the default here would send one nobody set.
        let (text, _) = read(&[("PASSWORD", "")], |env| env.text("PASSWORD", "default"));

        assert_eq!(text, "");
    }

    #[test]
    fn reports_every_bad_variable_rather_than_the_first() {
        // The property the collection exists for: one boot, one list, rather
        // than three restarts each naming the next mistake.
        let map = vars(&[("LEVEL", "loud"), ("PORT", "0"), ("GRACE", "600")]);
        let mut env = Env::new(&map);

        env.level("LEVEL");
        env.port("PORT", 3000);
        env.seconds("GRACE", 10, 300);
        let problems = env.finish().expect_err("expected a refusal").0;

        assert_eq!(problems.len(), 3);
        assert!(problems[0].starts_with("LEVEL: must be one of fatal, error, warn"));
        assert!(problems[1].starts_with("PORT: must be 1..=65535"));
        assert!(problems[2].starts_with("GRACE: must be 0..=300"));
    }

    #[test]
    fn rejects_a_port_at_either_end_of_its_range() {
        assert!(problems(&[("PORT", "0")], |env| env.port("PORT", 3000))[0].contains("got \"0\""));
        assert!(
            problems(&[("PORT", "65536")], |env| env.port("PORT", 3000))[0]
                .contains("got \"65536\"")
        );
    }

    #[test]
    fn rejects_seconds_past_the_ceiling_but_allows_none() {
        assert!(
            problems(&[("GRACE", "301")], |env| env.seconds("GRACE", 10, 300))[0]
                .contains("0..=300")
        );

        let (seconds, problems) = read(&[("GRACE", "0")], |env| env.seconds("GRACE", 10, 300));
        assert_eq!(seconds, 0, "zero is a choice");
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn rejects_a_host_that_is_not_an_address() {
        assert!(
            problems(&[("HOST", "localhost")], |env| env
                .address("HOST", "0.0.0.0"))[0]
                .contains("must be an IP address")
        );
    }

    #[test]
    fn rejects_a_url_that_is_not_one() {
        // A missing scheme is what neither `new URL()` nor this will take.
        assert!(
            problems(&[("URL", "http//localhost:8123")], |env| env.url("URL", ""))[0]
                .contains("must be a URL")
        );
    }

    #[test]
    fn takes_a_scheme_it_does_not_recognise() {
        // `new URL("clickhouse:8123")` is valid and so is this, and the point
        // is that the two agree rather than that the value is sensible.
        let problems = problems(&[("URL", "clickhouse:8123")], |env| env.url("URL", ""));

        assert!(problems.is_empty(), "as lenient as z.url(): {problems:?}");
    }

    #[test]
    fn maps_the_log_levels_pino_spells_differently() {
        // A deployment already setting either of these keeps working.
        let (fatal, _) = read(&[("LEVEL", "fatal")], |env| env.level("LEVEL"));
        let (silent, _) = read(&[("LEVEL", "silent")], |env| env.level("LEVEL"));

        assert_eq!(fatal, LevelFilter::ERROR);
        assert_eq!(silent, LevelFilter::OFF);
    }

    #[test]
    fn accepts_all_four_spellings_of_a_flag() {
        for (spelling, expected) in FLAGS {
            let (flag, problems) = read(&[("FLAG", spelling)], |env| env.flag("FLAG", false));

            assert_eq!(flag, expected, "FLAG={spelling}");
            assert!(problems.is_empty(), "FLAG={spelling}: {problems:?}");
        }
    }

    #[test]
    fn the_message_lists_the_problems_one_per_line() {
        let map = vars(&[("PORT", "0"), ("HOST", "nowhere")]);
        let mut env = Env::new(&map);

        env.address("HOST", "0.0.0.0");
        env.port("PORT", 3000);

        assert_eq!(
            env.finish().expect_err("expected a refusal").to_string(),
            "invalid environment configuration:\n  \
             HOST: must be an IP address, got \"nowhere\"\n  \
             PORT: must be 1..=65535, got \"0\""
        );
    }
}
