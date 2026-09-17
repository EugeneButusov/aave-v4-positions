//! One variable at a time, and what is wrong with the one that is.
//!
//! **A read is a read.** Every reader below takes `&self` and hands back a
//! `Result`, which is the shape
//! [crates.io's `crates_io_env_vars`](https://github.com/rust-lang/crates.io/blob/main/crates/crates_io_env_vars/src/lib.rs)
//! uses for the same job across forty-nine variables and sixteen files — free
//! functions returning `anyhow::Result`, and a config that assembles them with
//! `?`. Nothing here accumulates, so nothing here needs `&mut`.
//!
//! The version this replaces did accumulate, reporting every bad variable in one
//! boot. That behaviour came from the TypeScript rather than from Rust: Zod's
//! `safeParse` returns every issue at once, and `z.prettifyError` prints them one
//! per line, which is the message the Rust side was built to reproduce. Nothing
//! reads that message but a person, once, on a first deployment — and the price
//! was a `&mut` on eleven readers and on everything that borrowed one.
//!
//! **[`Invalid`] lives here rather than in an `error.rs`** of its own, which is
//! what the other crates in this workspace have. Nothing else produces it and it
//! carries no variants — splitting it out would buy a file boundary with a
//! constructor and an accessor, which is ceremony rather than structure.

use std::collections::HashMap;
use std::fmt;
use std::net::IpAddr;

use tracing::level_filters::LevelFilter;

/// The environment as a map, read one variable at a time.
pub struct Env<'a> {
    vars: &'a HashMap<String, String>,
}

/// The variable that could not be read, and why.
#[derive(Debug)]
pub struct Invalid {
    key: String,
    reason: String,
}

impl fmt::Display for Invalid {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self { key, reason } = self;

        write!(out, "invalid environment configuration: {key}: {reason}")
    }
}

impl std::error::Error for Invalid {}

impl Invalid {
    fn new(key: &str, reason: &str) -> Self {
        Self {
            key: key.to_owned(),
            reason: reason.to_owned(),
        }
    }
}

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
    /// name a bad variable without touching global state the other tests are
    /// running against.
    #[must_use]
    pub fn new(vars: &'a HashMap<String, String>) -> Self {
        Self { vars }
    }

    /// The one reader that cannot fail, which is why it is the one that returns
    /// no `Result`: an absent value is the default and any present value is
    /// itself.
    #[must_use]
    pub fn text(&self, key: &str, default: &str) -> String {
        self.raw(key).unwrap_or(default).to_owned()
    }

    /// A shared key, and one of two readers here with no default: a key every
    /// deployment shares is not a signature, so an absent one is a refusal
    /// rather than a fallback.
    ///
    /// **The value never reaches the message**, unlike every other reader here —
    /// that would put a signing key in the log of any deployment that mis-set
    /// it. Length only, in bytes, which is what a key is measured in.
    ///
    /// # Errors
    ///
    /// [`Invalid`] when the value is shorter than `minimum` bytes.
    pub fn secret(&self, key: &str, minimum: usize) -> Result<String, Invalid> {
        let value = self.raw(key).unwrap_or_default().to_owned();

        if value.len() < minimum {
            let length = value.len();
            return Err(Invalid::new(
                key,
                &format!("must be at least {minimum} bytes, got {length}"),
            ));
        }
        Ok(value)
    }

    /// No default, and no fallback worth having: the value names something
    /// outside this process, so guessing it produces a plausible answer to the
    /// wrong question.
    ///
    /// # Errors
    ///
    /// [`Invalid`] when the variable is absent or empty.
    pub fn required(&self, key: &str) -> Result<String, Invalid> {
        match self.raw(key) {
            Some(value) if !value.is_empty() => Ok(value.to_owned()),
            _ => Err(Invalid::new(key, "must be set")),
        }
    }

    /// As lenient as the WHATWG standard `url` implements, deliberately.
    ///
    /// `clickhouse:8123` is a valid URL — a scheme and a path — and is accepted.
    /// Requiring `http` here would be a stricter boot contract than a deployment
    /// expects, and the driver is the authority on its own URL anyway:
    /// `build_pool` parses this again with libpq's rules.
    ///
    /// # Errors
    ///
    /// [`Invalid`] when `url` will not parse it.
    pub fn url(&self, key: &str, default: &str) -> Result<String, Invalid> {
        let value = self.text(key, default);

        if url::Url::parse(&value).is_err() {
            return Err(Invalid::new(key, &format!("must be a URL, got {value:?}")));
        }
        Ok(value)
    }

    /// # Errors
    ///
    /// [`Invalid`] when the value is not an IP address. Stricter than
    /// `app.listen(port, host)`, which would resolve a hostname: every
    /// deployment of this sets an address, and a typo in one should fail at boot
    /// rather than bind somewhere unintended.
    pub fn address(&self, key: &str, default: &str) -> Result<IpAddr, Invalid> {
        let value = self.text(key, default);

        value
            .parse()
            .map_err(|_| Invalid::new(key, &format!("must be an IP address, got {value:?}")))
    }

    /// # Errors
    ///
    /// [`Invalid`] outside `1..=65535`. `u16` is the range; 0 is "any port",
    /// which is never what a service meant to be reachable at a known address
    /// was asking for.
    pub fn port(&self, key: &str, default: u16) -> Result<u16, Invalid> {
        let Some(value) = self.raw(key) else {
            return Ok(default);
        };

        match value.parse::<u16>() {
            Ok(port) if port > 0 => Ok(port),
            _ => Err(Invalid::new(
                key,
                &format!("must be 1..=65535, got {value:?}"),
            )),
        }
    }

    /// # Errors
    ///
    /// [`Invalid`] outside `0..=max`.
    pub fn seconds(&self, key: &str, default: u64, max: u64) -> Result<u64, Invalid> {
        let Some(value) = self.raw(key) else {
            return Ok(default);
        };

        match value.parse::<u64>() {
            Ok(seconds) if seconds <= max => Ok(seconds),
            _ => Err(Invalid::new(
                key,
                &format!("must be 0..={max}, got {value:?}"),
            )),
        }
    }

    /// # Errors
    ///
    /// [`Invalid`] outside `0.0..=1.0`.
    pub fn ratio(&self, key: &str, default: f64) -> Result<f64, Invalid> {
        let Some(value) = self.raw(key) else {
            return Ok(default);
        };

        match value.parse::<f64>() {
            Ok(ratio) if (0.0..=1.0).contains(&ratio) => Ok(ratio),
            _ => Err(Invalid::new(
                key,
                &format!("must be 0.0..=1.0, got {value:?}"),
            )),
        }
    }

    /// # Errors
    ///
    /// [`Invalid`] for anything but the four spellings in [`FLAGS`].
    pub fn flag(&self, key: &str, default: bool) -> Result<bool, Invalid> {
        let Some(value) = self.raw(key) else {
            return Ok(default);
        };

        self.one_of(key, value, &FLAGS)
    }

    /// # Errors
    ///
    /// [`Invalid`] for anything but the seven spellings in [`LEVELS`].
    pub fn level(&self, key: &str) -> Result<LevelFilter, Invalid> {
        let Some(value) = self.raw(key) else {
            return Ok(LevelFilter::INFO);
        };

        self.one_of(key, value, &LEVELS)
    }

    /// The spellings a caller accepts, and what each one means to it. [`flag`]
    /// and [`level`] are this with the table built in; a table nothing else
    /// shares stays with the code that gives it meaning, so this crate keeps
    /// knowing nothing about what a service is.
    ///
    /// # Errors
    ///
    /// [`Invalid`] for anything not in `table`.
    ///
    /// [`flag`]: Self::flag
    /// [`level`]: Self::level
    pub fn choice<T: Copy>(
        &self,
        key: &str,
        table: &[(&str, T)],
        default: T,
    ) -> Result<T, Invalid> {
        let Some(value) = self.raw(key) else {
            return Ok(default);
        };

        self.one_of(key, value, table)
    }

    /// Borrowed from the map rather than from `self`, so a value can be read
    /// and then named in an error about it.
    fn raw(&self, key: &str) -> Option<&'a str> {
        // An empty value is a value: `CLICKHOUSE_PASSWORD=` means no password,
        // and treating it as absent would substitute a default nobody asked for.
        self.vars.get(key).map(String::as_str)
    }

    fn one_of<T: Copy>(&self, key: &str, value: &str, table: &[(&str, T)]) -> Result<T, Invalid> {
        table
            .iter()
            .find(|(spelling, _)| *spelling == value)
            .map(|(_, mapped)| *mapped)
            .ok_or_else(|| {
                let allowed: Vec<_> = table.iter().map(|(spelling, _)| *spelling).collect();

                Invalid::new(
                    key,
                    &format!("must be one of {}, got {value:?}", allowed.join(", ")),
                )
            })
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

    /// Reads one key with `read`, and hands back whatever it answered.
    fn read<T>(pairs: &[(&str, &str)], read: impl FnOnce(&Env<'_>) -> T) -> T {
        read(&Env::new(&vars(pairs)))
    }

    /// The refusal a bad variable earns, as a deployment would read it.
    fn refusal<T: fmt::Debug>(
        pairs: &[(&str, &str)],
        reader: impl FnOnce(&Env<'_>) -> Result<T, Invalid>,
    ) -> String {
        read(pairs, reader)
            .expect_err("expected a refusal")
            .to_string()
    }

    #[test]
    fn an_absent_variable_is_its_default_and_not_a_problem() {
        assert_eq!(read(&[], |env| env.port("PORT", 3000)).unwrap(), 3000);
    }

    #[test]
    fn an_empty_value_is_a_value_and_not_an_absence() {
        // A container started with CLICKHOUSE_SKIP_USER_SETUP has no password,
        // and falling back to the default here would send one nobody set.
        let text = read(&[("PASSWORD", "")], |env| env.text("PASSWORD", "default"));

        assert_eq!(text, "");
    }

    #[test]
    fn the_message_names_the_variable_and_says_what_was_wanted() {
        assert_eq!(
            refusal(&[("PORT", "0")], |env| env.port("PORT", 3000)),
            "invalid environment configuration: PORT: must be 1..=65535, got \"0\""
        );
    }

    #[test]
    fn rejects_a_port_at_either_end_of_its_range() {
        assert!(refusal(&[("PORT", "0")], |env| env.port("PORT", 3000)).contains("got \"0\""));
        assert!(
            refusal(&[("PORT", "65536")], |env| env.port("PORT", 3000)).contains("got \"65536\"")
        );
    }

    #[test]
    fn rejects_seconds_past_the_ceiling_but_allows_none() {
        assert!(
            refusal(&[("GRACE", "301")], |env| env.seconds("GRACE", 10, 300)).contains("0..=300")
        );

        let seconds = read(&[("GRACE", "0")], |env| env.seconds("GRACE", 10, 300)).unwrap();

        assert_eq!(seconds, 0, "zero is a choice");
    }

    #[test]
    fn rejects_a_host_that_is_not_an_address() {
        assert!(
            refusal(&[("HOST", "localhost")], |env| env
                .address("HOST", "0.0.0.0"))
            .contains("must be an IP address")
        );
    }

    #[test]
    fn rejects_a_url_that_is_not_one() {
        // A missing scheme is what neither `new URL()` nor this will take.
        assert!(
            refusal(&[("URL", "http//localhost:8123")], |env| env.url("URL", ""))
                .contains("must be a URL")
        );
    }

    #[test]
    fn takes_every_spelling_of_a_level_and_of_a_flag() {
        for (spelling, expected) in LEVELS {
            assert_eq!(
                read(&[("LEVEL", spelling)], |env| env.level("LEVEL")).unwrap(),
                expected
            );
        }
        for (spelling, expected) in FLAGS {
            assert_eq!(
                read(&[("FLAG", spelling)], |env| env.flag("FLAG", !expected)).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn an_absent_level_is_info_and_an_absent_flag_is_its_default() {
        assert_eq!(
            read(&[], |env| env.level("LEVEL")).unwrap(),
            LevelFilter::INFO
        );
        assert!(read(&[], |env| env.flag("FLAG", true)).unwrap());
    }

    #[test]
    fn rejects_a_level_and_a_flag_it_does_not_know() {
        assert!(refusal(&[("LEVEL", "loud")], |env| env.level("LEVEL")).contains("must be one of"));
        assert!(
            refusal(&[("FLAG", "yes")], |env| env.flag("FLAG", false)).contains("must be one of")
        );
    }

    #[test]
    fn a_secret_shorter_than_the_minimum_is_refused_without_being_logged() {
        let refusal = refusal(&[("SECRET", "too-short-to-sign-anything")], |env| {
            env.secret("SECRET", 32)
        });

        assert!(
            refusal.contains("SECRET: must be at least 32 bytes, got 26"),
            "{refusal}"
        );
        assert!(
            !refusal.contains("too-short"),
            "the value reached the message"
        );
    }

    #[test]
    fn a_secret_at_the_minimum_is_long_enough() {
        let key = "a".repeat(32);

        assert_eq!(
            read(&[("SECRET", &key)], |env| env.secret("SECRET", 32)).unwrap(),
            key
        );
    }

    #[test]
    fn a_required_variable_is_refused_when_absent_and_when_empty() {
        // The empty case is the one worth a test: every other reader here
        // treats an empty value as a value, and this is the reader where that
        // rule would hand back a name nothing can be grouped by.
        for pairs in [vec![], vec![("NAME", "")]] {
            assert!(refusal(&pairs, |env| env.required("NAME")).ends_with("NAME: must be set"));
        }
    }

    #[test]
    fn a_required_variable_that_is_set_is_read_as_it_stands() {
        assert_eq!(
            read(&[("NAME", "api-rust")], |env| env.required("NAME")).unwrap(),
            "api-rust"
        );
    }

    #[test]
    fn a_choice_maps_the_spellings_its_caller_offered() {
        const SIDES: [(&str, u8); 2] = [("heads", 0), ("tails", 1)];

        assert_eq!(
            read(&[("SIDE", "tails")], |env| env.choice("SIDE", &SIDES, 0)).unwrap(),
            1
        );
        assert_eq!(read(&[], |env| env.choice("SIDE", &SIDES, 0)).unwrap(), 0);
    }

    #[test]
    fn a_choice_names_the_spellings_it_would_have_taken() {
        const SIDES: [(&str, u8); 2] = [("heads", 0), ("tails", 1)];

        assert!(
            refusal(&[("SIDE", "edge")], |env| env.choice("SIDE", &SIDES, 0))
                .ends_with(r#"SIDE: must be one of heads, tails, got "edge""#)
        );
    }

    #[test]
    fn a_ratio_holds_to_its_ends_and_refuses_what_is_past_them() {
        let zero = read(&[("SHARE", "0")], |env| env.ratio("SHARE", 1.0)).unwrap();
        assert!(zero.abs() < f64::EPSILON, "{zero}");

        let one = read(&[("SHARE", "1.0")], |env| env.ratio("SHARE", 0.0)).unwrap();
        assert!((one - 1.0).abs() < f64::EPSILON, "{one}");

        for past in ["-0.1", "1.1", "half"] {
            assert!(
                refusal(&[("SHARE", past)], |env| env.ratio("SHARE", 1.0))
                    .ends_with(&format!(r#"SHARE: must be 0.0..=1.0, got "{past}""#))
            );
        }
    }
}
