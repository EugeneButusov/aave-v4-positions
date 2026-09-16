//! One variable at a time, and everything wrong with the lot of them.
//!
//! **Every error at once, not the first**: a deployment with three variables
//! wrong learns all three on the first boot instead of one per crash loop. Every
//! reader below returns a usable value *and* records a problem, so parsing
//! continues past a bad variable and [`Env::finish`] reports the lot.
//!
//! **Which is why it is hand-rolled.** The derive-based readers stop at the
//! first bad field — measured, against `API_PORT=0`, `API_HOST=nowhere` and
//! `SHUTDOWN_GRACE_SECONDS=600`, which they report one of. The environment is
//! taken as a map rather than read, so a case can name three bad variables
//! without touching what the other tests are running against.
//!
//! **[`Invalid`] lives here rather than in an `error.rs`** of its own, which is
//! what the other crates in this workspace have. Nothing else produces it and it
//! carries no variants — splitting it out would buy a file boundary with a
//! constructor and an accessor, which is ceremony rather than structure.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;

use tracing::level_filters::LevelFilter;

/// The environment as a map, and what has been wrong with it so far.
pub struct Env<'a> {
    vars: &'a HashMap<String, String>,
    errors: Vec<String>,
}

/// Everything wrong with the environment, in the order the variables are read.
#[derive(Debug)]
pub struct Invalid(Vec<String>);

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

/// The seven spellings a deployment may already be setting.
///
/// `fatal` and `silent` have no `tracing` counterpart of their own — the first
/// is an error, the second the absence of a level — and mapping them is cheaper
/// than a release note nobody reads at three in the morning.
const LEVELS: [(&str, LevelFilter); 7] = [
    ("fatal", LevelFilter::ERROR),
    ("error", LevelFilter::ERROR),
    ("warn", LevelFilter::WARN),
    ("info", LevelFilter::INFO),
    ("debug", LevelFilter::DEBUG),
    ("trace", LevelFilter::TRACE),
    ("silent", LevelFilter::OFF),
];

/// The four spellings a boolean may arrive as.
const FLAGS: [(&str, bool); 4] = [("true", true), ("1", true), ("false", false), ("0", false)];

impl<'a> Env<'a> {
    /// Takes the environment as a map rather than reading it, so a case can
    /// name three bad variables without touching global state the other tests
    /// are running against.
    pub fn new(vars: &'a HashMap<String, String>) -> Self {
        Self {
            vars,
            errors: Vec::new(),
        }
    }

    /// # Errors
    ///
    /// [`Invalid`], listing every variable that could not be read.
    pub fn finish(self) -> Result<(), Invalid> {
        if self.errors.is_empty() {
            Ok(())
        } else {
            Err(Invalid(self.errors))
        }
    }

    pub fn text(&self, key: &str, default: &str) -> String {
        self.raw(key).unwrap_or(default).to_owned()
    }

    /// A shared key, and the only reader here with no default: a key every
    /// deployment shares is not a signature, so an absent one is a problem
    /// rather than a fallback. The empty string handed back only lets the
    /// remaining variables still be read.
    ///
    /// **The value never reaches the message**, unlike every other reader here —
    /// that would put a signing key in the log of any deployment that mis-set
    /// it. Length only, in bytes, which is what a key is measured in.
    pub fn secret(&mut self, key: &str, minimum: usize) -> String {
        let value = self.raw(key).unwrap_or_default().to_owned();

        if value.len() < minimum {
            let length = value.len();
            self.reject(
                key,
                &format!("must be at least {minimum} bytes, got {length}"),
            );
        }
        value
    }

    /// No default, and no fallback worth having: the value names something
    /// outside this process, so guessing it produces a plausible answer to the
    /// wrong question. The empty string handed back only lets the remaining
    /// variables still be read.
    pub fn required(&mut self, key: &str) -> String {
        let value = self.raw(key).unwrap_or_default().to_owned();

        if value.is_empty() {
            self.reject(key, "must be set");
        }
        value
    }

    /// As lenient as the WHATWG standard `url` implements, deliberately.
    ///
    /// `clickhouse:8123` is a valid URL — a scheme and a path — and is accepted.
    /// Requiring `http` here would be a stricter boot contract than a deployment
    /// expects, and the driver is the authority on its own URL anyway:
    /// `build_pool` parses this again with libpq's rules.
    pub fn url(&mut self, key: &str, default: &str) -> String {
        let value = self.text(key, default);
        if url::Url::parse(&value).is_err() {
            self.reject(key, &format!("must be a URL, got {value:?}"));
        }
        value
    }

    pub fn address(&mut self, key: &str, default: &str) -> IpAddr {
        let value = self.text(key, default);
        value.parse().unwrap_or_else(|_| {
            // Stricter than `app.listen(port, host)`, which would resolve a
            // hostname. Every deployment of this sets an address, and a typo in
            // one should fail at boot rather than bind somewhere unintended.
            self.reject(key, &format!("must be an IP address, got {value:?}"));
            IpAddr::from([0, 0, 0, 0])
        })
    }

    pub fn port(&mut self, key: &str, default: u16) -> u16 {
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

    pub fn seconds(&mut self, key: &str, default: u64, max: u64) -> u64 {
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

    pub fn ratio(&mut self, key: &str, default: f64) -> f64 {
        let Some(value) = self.raw(key) else {
            return default;
        };

        match value.parse::<f64>() {
            Ok(ratio) if (0.0..=1.0).contains(&ratio) => ratio,
            _ => {
                self.reject(key, &format!("must be 0.0..=1.0, got {value:?}"));
                default
            }
        }
    }

    pub fn flag(&mut self, key: &str, default: bool) -> bool {
        let Some(value) = self.raw(key) else {
            return default;
        };

        self.one_of(key, value, &FLAGS).unwrap_or(default)
    }

    pub fn level(&mut self, key: &str) -> LevelFilter {
        let Some(value) = self.raw(key) else {
            return LevelFilter::INFO;
        };

        self.one_of(key, value, &LEVELS)
            .unwrap_or(LevelFilter::INFO)
    }

    /// The spellings a caller accepts, and what each one means to it. `flag` and
    /// `level` are this with the table built in; a table nothing else shares
    /// stays with the code that gives it meaning, so this crate keeps knowing
    /// nothing about what a service is.
    pub fn choice<T: Copy>(&mut self, key: &str, table: &[(&str, T)], default: T) -> T {
        let Some(value) = self.raw(key) else {
            return default;
        };

        self.one_of(key, value, table).unwrap_or(default)
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
    //! The readers on their own terms, with keys that mean nothing to anyone.
    //! What a variable is actually called, and which reader it gets, is a
    //! consumer's `config` to prove.
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
    fn maps_the_log_levels_that_have_no_counterpart() {
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
    fn a_secret_has_no_default_and_an_absent_one_is_a_problem() {
        let (secret, problems) = read(&[], |env| env.secret("SECRET", 32));

        assert_eq!(secret, "", "usable enough for the remaining reads");
        assert_eq!(problems, ["SECRET: must be at least 32 bytes, got 0"]);
    }

    #[test]
    fn a_secret_never_appears_in_the_problem_it_causes() {
        // Every other reader quotes the value it refused. This one is a signing
        // key, and the refusal goes to the log of whatever mis-set it.
        let problems = problems(&[("SECRET", "too-short-but-still-a-key")], |env| {
            env.secret("SECRET", 32)
        });

        assert_eq!(problems, ["SECRET: must be at least 32 bytes, got 25"]);
    }

    #[test]
    fn a_secret_at_the_minimum_is_long_enough() {
        let key = "a".repeat(32);
        let (secret, problems) = read(&[("SECRET", &key)], |env| env.secret("SECRET", 32));

        assert_eq!(secret, key);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_required_variable_is_refused_when_absent_and_when_empty() {
        // The empty case is the one worth a test: every other reader here
        // treats an empty value as a value, and this is the reader where that
        // rule would hand back a name nothing can be grouped by.
        assert_eq!(
            problems(&[], |env| env.required("NAME")),
            ["NAME: must be set"]
        );
        assert_eq!(
            problems(&[("NAME", "")], |env| env.required("NAME")),
            ["NAME: must be set"]
        );
    }

    #[test]
    fn a_required_variable_that_is_set_is_read_as_it_stands() {
        let (name, problems) = read(&[("NAME", "api-rust")], |env| env.required("NAME"));

        assert_eq!(name, "api-rust");
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_choice_maps_the_spellings_its_caller_offered() {
        const SIDES: [(&str, u8); 2] = [("heads", 0), ("tails", 1)];

        let (side, problems) = read(&[("SIDE", "tails")], |env| env.choice("SIDE", &SIDES, 0));

        assert_eq!(side, 1);
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn a_choice_names_the_spellings_it_would_have_taken() {
        const SIDES: [(&str, u8); 2] = [("heads", 0), ("tails", 1)];

        assert_eq!(
            problems(&[("SIDE", "edge")], |env| env.choice("SIDE", &SIDES, 0)),
            [r#"SIDE: must be one of heads, tails, got "edge""#]
        );
    }

    #[test]
    fn a_ratio_holds_to_its_ends_and_refuses_what_is_past_them() {
        let (zero, bad) = read(&[("SHARE", "0")], |env| env.ratio("SHARE", 1.0));
        assert!(zero.abs() < f64::EPSILON, "{zero}");
        assert!(bad.is_empty(), "{bad:?}");

        let (one, bad) = read(&[("SHARE", "1.0")], |env| env.ratio("SHARE", 0.0));
        assert!((one - 1.0).abs() < f64::EPSILON, "{one}");
        assert!(bad.is_empty(), "{bad:?}");

        for past in ["-0.1", "1.1", "half"] {
            assert_eq!(
                problems(&[("SHARE", past)], |env| env.ratio("SHARE", 1.0)),
                [format!(r#"SHARE: must be 0.0..=1.0, got "{past}""#)]
            );
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
