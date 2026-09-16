//! What this process is set up to be, parsed once, before anything else.
//!
//! The list below is the whole of it: every variable this service honours, the
//! reader it goes through and the default it falls back to. Reading and
//! validating are [`env`](mod@env)'s job — a port's range and a level's spelling mean
//! nothing here. This is the mapping from a container's
//! environment to a running service, and it is the only place that mapping
//! exists.
//!
//! **Every variable here has a reader**, because a field nobody uses is
//! `dead_code` and the workspace denies it. So the list grows with the code that
//! needs it — `API_GLOBAL_PREFIX` and the cursor secret arrived with the route,
//! the `OTEL_*` group arrives with `telemetry` — and it cannot drift from what
//! the process honours.
//!
//! **Two variables have no default**, and both refuse rather than fall back. A
//! cursor signing key every deployment shares is not a signature, so an unset
//! `POSITIONS_CURSOR_SECRET` is a process that serves forgeable cursors. And
//! every signal is grouped by `service.name`, so an unset `OTEL_SERVICE_NAME` is
//! telemetry that is present, plausible and impossible to attribute — noticed
//! for the first time during an incident. `OTEL_SDK_DISABLED=true` is how a
//! process says it wants none of it.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use env::{Env, Invalid, Source};
use telemetry::Sampling;

use crate::positions::MIN_SECRET_BYTES;

/// A threshold past a day never fires on a chain with twelve-second blocks, so
/// an unbounded one is a number with no wrong value.
///
/// **A bound, not a units check.** It catches `300000` — the price default in
/// milliseconds — and not `60000`, the sync default in milliseconds, under a day
/// and just as wrong. The name carries the unit; this only stops the absurd.
const MAX_STALENESS_SECONDS: u64 = 86_400;

/// The six the specification spells, and no name of our own: an operator who
/// knows OpenTelemetry should not have to learn ours.
const SAMPLERS: [(&str, Sampling); 6] = [
    ("always_on", Sampling::AlwaysOn),
    ("always_off", Sampling::AlwaysOff),
    ("traceidratio", Sampling::TraceIdRatio),
    ("parentbased_always_on", Sampling::ParentBasedAlwaysOn),
    ("parentbased_always_off", Sampling::ParentBasedAlwaysOff),
    (
        "parentbased_traceidratio",
        Sampling::ParentBasedTraceIdRatio,
    ),
];

pub(crate) struct Config {
    pub(crate) telemetry: telemetry::Settings,
    pub(crate) host: IpAddr,
    pub(crate) port: u16,
    pub(crate) grace: Duration,
    /// What the versioned routes are mounted under. The probes are not: an
    /// orchestrator's check is not part of the API's versioned surface, and
    /// every compose healthcheck already asks for `/health/ready`.
    pub(crate) prefix: String,

    /// Outside the prefix, for the reason above. **Always served**: a contract
    /// absent from the environment people call is not much of a contract.
    pub(crate) docs_path: String,

    /// On disk, copied in by the image. Absent is the document with no viewer
    /// in front of it, which is what a `cargo run` outside the image gets.
    pub(crate) docs_assets: String,
    pub(crate) cursor_secret: String,
    pub(crate) staleness: Staleness,
    pub(crate) clickhouse: clickhouse_client::Config,
    pub(crate) postgres_url: String,
}

/// How old a number may be before a page says so.
///
/// **Two rather than one**, because the clocks run at different rates: the
/// indexer advances every block, the oracle is read every minute. Neither is the
/// indexer's `INDEXER_STALL_THRESHOLD_MS`, which decides whether to drain a pod.
/// Zero is a choice, and it means every page says so.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Staleness {
    pub(crate) sync: u64,

    /// **How long since we last read the oracle, never since a feed moved** —
    /// §7.5's trap. An hour without an `AnswerUpdated` is ordinary Chainlink
    /// behaviour, so a threshold from feed cadence flags healthy feeds forever.
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
            telemetry: telemetry(&mut env),
            host: env.address("API_HOST", "0.0.0.0"),
            port: env.port("API_PORT", 3000),
            grace: Duration::from_secs(env.seconds("SHUTDOWN_GRACE_SECONDS", 10, 300)),
            prefix: env.text("API_GLOBAL_PREFIX", "api"),
            docs_path: env.text("API_DOCS_PATH", "docs"),
            docs_assets: env.text("API_DOCS_ASSETS", "/usr/share/api/docs"),
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

/// Everything the exporters and the formatter are set up from.
///
/// **The `OTEL_*` spellings are the specification's**, which is the whole reason
/// they can be read here alongside everything else: the service this replaces
/// had to let its SDK read them before its own configuration existed, and that
/// exception does not survive the port.
fn telemetry(env: &mut Env<'_>) -> telemetry::Settings {
    let level = env.level("LOG_LEVEL");
    let pretty = env.flag("LOG_PRETTY", false);
    let disabled = env.flag("OTEL_SDK_DISABLED", false);

    telemetry::Settings {
        service: if disabled {
            String::new()
        } else {
            env.required("OTEL_SERVICE_NAME")
        },
        endpoint: env.url("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4318"),
        sampler: env.choice(
            "OTEL_TRACES_SAMPLER",
            &SAMPLERS,
            Sampling::ParentBasedAlwaysOn,
        ),
        ratio: env.ratio("OTEL_TRACES_SAMPLER_ARG", 1.0),
        level,
        pretty,
        disabled,
    }
}

#[cfg(test)]
mod tests {
    //! The mapping, not the readers: which variable reaches which field, and
    //! what a deployment that sets none of them gets. [`env`](mod@env) proves that a
    //! port is a port.

    use tracing::level_filters::LevelFilter;

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

    /// Every case that is not about them still has to set the two variables
    /// with no default, because without either nothing else parses.
    fn configured(pairs: &[(&str, &str)]) -> Result<Config, Invalid> {
        let mut all = pairs.to_vec();
        all.push(SECRET);
        all.push(("OTEL_SERVICE_NAME", "api-rust"));

        parse(&all)
    }

    #[test]
    fn an_empty_environment_is_the_local_defaults() {
        let config = configured(&[]).expect("defaults should stand alone");

        assert_eq!(config.telemetry.level, LevelFilter::INFO);
        assert!(!config.telemetry.pretty);
        assert!(!config.telemetry.disabled);
        assert_eq!(config.telemetry.endpoint, "http://localhost:4318");
        assert_eq!(config.telemetry.sampler, Sampling::ParentBasedAlwaysOn);
        assert!((config.telemetry.ratio - 1.0).abs() < f64::EPSILON);
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
        assert_eq!(config.docs_path, "docs");
        assert_eq!(config.docs_assets, "/usr/share/api/docs");
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
            ("OTEL_SDK_DISABLED", "false"),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://telemetry:4318"),
            ("OTEL_TRACES_SAMPLER", "parentbased_traceidratio"),
            ("OTEL_TRACES_SAMPLER_ARG", "0.25"),
        ])
        .expect("every value is valid");

        assert_eq!(config.telemetry.level, LevelFilter::DEBUG);
        assert!(config.telemetry.pretty);
        assert_eq!(config.telemetry.service, "api-rust");
        assert_eq!(config.telemetry.endpoint, "http://telemetry:4318");
        assert_eq!(config.telemetry.sampler, Sampling::ParentBasedTraceIdRatio);
        assert!((config.telemetry.ratio - 0.25).abs() < f64::EPSILON);
        assert!(!config.telemetry.disabled);
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
    fn refuses_to_boot_unnamed_while_telemetry_is_on() {
        // Every signal is grouped by `service.name`. Defaulting it produces
        // telemetry that is present, plausible and attributed to nothing, which
        // is the shape that is only ever noticed during an incident.
        let refusal = parse(&[SECRET])
            .err()
            .expect("expected a refusal")
            .to_string();

        assert!(
            refusal.contains("OTEL_SERVICE_NAME: must be set"),
            "{refusal}"
        );
    }

    #[test]
    fn a_process_that_wants_no_telemetry_needs_no_name_for_it() {
        // The switch a deployment already knows, rather than a name of ours.
        let config =
            parse(&[SECRET, ("OTEL_SDK_DISABLED", "true")]).expect("disabled is a complete answer");

        assert!(config.telemetry.disabled);
        assert_eq!(config.telemetry.service, "");
    }

    #[test]
    fn takes_the_samplers_the_specification_spells_and_no_others() {
        let sampler = |value| {
            configured(&[("OTEL_TRACES_SAMPLER", value)]).map(|config| config.telemetry.sampler)
        };

        assert_eq!(sampler("always_off").expect("spelled"), Sampling::AlwaysOff);
        assert_eq!(
            sampler("traceidratio").expect("spelled"),
            Sampling::TraceIdRatio
        );

        let refusal = sampler("parentbased_jaeger_remote")
            .expect_err("expected a refusal")
            .to_string();

        assert!(
            refusal.contains("OTEL_TRACES_SAMPLER: must be one of"),
            "{refusal}"
        );
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
