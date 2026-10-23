//! What this process is set up to be, parsed once, before anything else.
//!
//! The only mapping from a container's environment to a running service.
//! Reading and validating are [`env`](mod@env)'s job.
//!
//! `POSITIONS_CURSOR_SECRET` and `OTEL_SERVICE_NAME` have no default and
//! refuse rather than fall back; `OTEL_SDK_DISABLED=true` asks for no
//! telemetry at all.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use env::{Env, Invalid, Source};
use telemetry::{Endpoints, Sampling};

use crate::positions::MIN_SECRET_BYTES;

/// A bound, not a units check: it catches `300000` and not `60000`, which is
/// just as wrong. The name carries the unit; this only stops the absurd.
const MAX_STALENESS_SECONDS: u64 = 86_400;

/// A units check on every deadline, like the one above. Catches a minute typed
/// as milliseconds and nothing subtler.
const MAX_DEADLINE_MS: u64 = 600_000;

/// The six the specification spells, and no name of our own.
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

    /// Outside the prefix, and always served.
    pub(crate) docs_path: String,

    /// On disk, copied in by the image. Absent is the document with no viewer
    /// in front of it, which is what a `cargo run` gets.
    pub(crate) docs_assets: String,
    pub(crate) cursor_secret: String,
    pub(crate) staleness: Staleness,
    pub(crate) clickhouse: clickhouse_client::Config,
    pub(crate) postgres_url: String,
    pub(crate) postgres_limits: postgres::Limits,
}

/// How old a number may be before a page says so.
///
/// Two rather than one, because the indexer advances every block and the oracle
/// is read every minute. Neither is the indexer's `INDEXER_STALL_THRESHOLD_MS`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Staleness {
    pub(crate) sync: u64,

    /// How long since we last read the oracle, never since a feed moved
    /// (§7.5): an hour without an `AnswerUpdated` is ordinary Chainlink.
    pub(crate) price: u64,
}

impl Config {
    /// Which files [`Source`] reads is decided by `APP_ENV`, not by how the
    /// binary was built.
    ///
    /// # Errors
    ///
    /// [`Invalid`], naming the first variable that could not be read.
    pub(crate) fn from_env() -> Result<Self, Invalid> {
        Self::parse(&Source::from_env().read())
    }

    /// Takes the environment as a map, so a case touches no global state.
    fn parse(vars: &HashMap<String, String>) -> Result<Self, Invalid> {
        let env = Env::new(vars);

        Ok(Self {
            telemetry: telemetry(&env)?,
            host: env.address("API_HOST", "0.0.0.0")?,
            port: env.port("API_PORT", 3000)?,
            grace: Duration::from_secs(env.seconds("SHUTDOWN_GRACE_SECONDS", 10, 300)?),
            prefix: env.text("API_GLOBAL_PREFIX", "api"),
            docs_path: env.text("API_DOCS_PATH", "docs"),
            docs_assets: env.text("API_DOCS_ASSETS", "/usr/share/api/docs"),
            cursor_secret: env.secret("POSITIONS_CURSOR_SECRET", MIN_SECRET_BYTES)?,
            staleness: Staleness {
                sync: env.seconds("API_SYNC_STALE_AFTER_SECONDS", 60, MAX_STALENESS_SECONDS)?,
                price: env.seconds("API_PRICE_STALE_AFTER_SECONDS", 300, MAX_STALENESS_SECONDS)?,
            },
            clickhouse: clickhouse_client::Config {
                url: env.url("CLICKHOUSE_URL", "http://localhost:8123")?,
                transport_security: if env.flag("CLICKHOUSE_ALLOW_INSECURE_HTTP", false)? {
                    clickhouse_client::TransportSecurity::AllowInsecureHttp
                } else {
                    clickhouse_client::TransportSecurity::RequireTls
                },
                database: env.text("CLICKHOUSE_DATABASE", "default"),
                user: env.text("CLICKHOUSE_USER", "default"),
                // Empty is legitimate: `CLICKHOUSE_SKIP_USER_SETUP` leaves no
                // password, which is how the test and CI instances run.
                password: env.text("CLICKHOUSE_PASSWORD", ""),
                max_execution: env.millis(
                    "CLICKHOUSE_MAX_EXECUTION_MS",
                    10_000,
                    1,
                    MAX_DEADLINE_MS,
                )?,
            },
            postgres_url: env.url(
                "POSTGRES_URL",
                "postgres://postgres@localhost:5432/postgres",
            )?,
            postgres_limits: postgres::Limits {
                max_size: env.size("POSTGRES_MAX_CONNECTIONS", 4, 1, 1_000)?,
                wait: env.millis("POSTGRES_WAIT_MS", 250, 1, MAX_DEADLINE_MS)?,
                create: env.millis("POSTGRES_CONNECT_MS", 2_000, 1, MAX_DEADLINE_MS)?,
                recycle: env.millis("POSTGRES_RECYCLE_MS", 1_000, 1, MAX_DEADLINE_MS)?,
                // Always bounded here; `Unbounded` is a caller's decision.
                statement: postgres::Deadline::After(env.millis(
                    "POSTGRES_STATEMENT_MS",
                    5_000,
                    1,
                    MAX_DEADLINE_MS,
                )?),
            },
        })
    }
}

/// OTLP/HTTP's own paths, over the base address the specification says
/// `OTEL_EXPORTER_OTLP_ENDPOINT` carries.
///
/// The same composition the SDK performs when it reads that variable itself,
/// done here because `telemetry` configures the exporters programmatically and
/// `with_endpoint` is then taken verbatim — given the base alone every batch
/// posts to `/` and is 404ed in silence. It lives in this file because mapping
/// an environment onto a running service is what this file is.
fn endpoints(base: &str) -> Endpoints {
    let base = base.trim_end_matches('/');

    Endpoints {
        traces: format!("{base}/v1/traces"),
        metrics: format!("{base}/v1/metrics"),
        logs: format!("{base}/v1/logs"),
    }
}

fn telemetry(env: &Env<'_>) -> Result<telemetry::Settings, Invalid> {
    let disabled = env.flag("OTEL_SDK_DISABLED", false)?;

    Ok(telemetry::Settings {
        service: if disabled {
            String::new()
        } else {
            env.required("OTEL_SERVICE_NAME")?
        },
        endpoints: endpoints(&env.url("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4318")?),
        sampler: env.choice(
            "OTEL_TRACES_SAMPLER",
            &SAMPLERS,
            Sampling::ParentBasedAlwaysOn,
        )?,
        ratio: env.ratio("OTEL_TRACES_SAMPLER_ARG", 1.0)?,
        level: env.level("LOG_LEVEL")?,
        pretty: env.flag("LOG_PRETTY", false)?,
        disabled,
    })
}

#[cfg(test)]
mod tests {
    //! The mapping, not the readers: which variable reaches which field, and
    //! what a deployment that sets none of them gets.

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

    /// The two variables with no default, which every other case needs set.
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
        assert_eq!(
            config.telemetry.endpoints.traces,
            "http://localhost:4318/v1/traces"
        );
        assert_eq!(config.telemetry.sampler, Sampling::ParentBasedAlwaysOn);
        assert!((config.telemetry.ratio - 1.0).abs() < f64::EPSILON);
        assert_eq!(config.host, IpAddr::from([0, 0, 0, 0]));
        assert_eq!(config.port, 3000);
        assert_eq!(config.grace, Duration::from_secs(10));
        assert_eq!(config.clickhouse.url, "http://localhost:8123");
        assert_eq!(
            config.clickhouse.transport_security,
            clickhouse_client::TransportSecurity::RequireTls
        );
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
        // A shared default key is not a signature: the alternative to this
        // refusal is a process serving forgeable cursors.
        // The service name is set because telemetry is read first.
        let refusal = parse(&[("OTEL_SERVICE_NAME", "api-rust")])
            .err()
            .expect("expected a refusal")
            .to_string();

        assert!(refusal.contains("POSITIONS_CURSOR_SECRET"), "{refusal}");
        assert!(!refusal.contains("a-test-key"), "the key reached the log");
    }

    #[test]
    fn each_signal_gets_the_path_the_collector_is_listening_for() {
        let posted = endpoints("http://telemetry:4318");

        assert_eq!(posted.traces, "http://telemetry:4318/v1/traces");
        assert_eq!(posted.metrics, "http://telemetry:4318/v1/metrics");
        assert_eq!(posted.logs, "http://telemetry:4318/v1/logs");
    }

    #[test]
    fn a_trailing_slash_does_not_become_a_second_one() {
        assert_eq!(
            endpoints("http://telemetry:4318/").logs,
            "http://telemetry:4318/v1/logs"
        );
    }

    #[test]
    fn the_first_bad_variable_in_reading_order_is_the_one_reported() {
        // Fields evaluate in source order, so this pins the order a deployment
        // reads its refusals in. Both are wrong; the one named is read first.
        let refusal = configured(&[("API_PORT", "0"), ("SHUTDOWN_GRACE_SECONDS", "600")])
            .err()
            .expect("expected a refusal")
            .to_string();

        assert!(
            refusal.ends_with("API_PORT: must be 1..=65535, got \"0\""),
            "{refusal}"
        );
    }

    #[test]
    fn bounds_a_staleness_threshold_without_pretending_to_check_its_units() {
        // Both halves: the price default in milliseconds is refused, the sync
        // default in milliseconds is taken and just as wrong.
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
            ("CLICKHOUSE_ALLOW_INSECURE_HTTP", "true"),
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
        assert_eq!(
            config.telemetry.endpoints.metrics,
            "http://telemetry:4318/v1/metrics"
        );
        assert_eq!(config.telemetry.sampler, Sampling::ParentBasedTraceIdRatio);
        assert!((config.telemetry.ratio - 0.25).abs() < f64::EPSILON);
        assert!(!config.telemetry.disabled);
        assert_eq!(config.host, IpAddr::from([127, 0, 0, 1]));
        assert_eq!(config.port, 8080);
        assert_eq!(config.grace, Duration::from_secs(5));
        assert_eq!(config.clickhouse.database, "aave");
        assert_eq!(config.clickhouse.password, "hunter2");
        assert_eq!(
            config.clickhouse.transport_security,
            clickhouse_client::TransportSecurity::AllowInsecureHttp
        );
        assert_eq!(config.prefix, "gateway");
        assert_eq!(config.cursor_secret, SECRET.1);
        assert_eq!(config.staleness.sync, 30);
        assert_eq!(config.staleness.price, 900);
    }

    #[test]
    fn refuses_to_boot_unnamed_while_telemetry_is_on() {
        // Defaulting it produces telemetry attributed to nothing.
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
