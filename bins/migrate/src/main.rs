//! Applies both schemas, then exits.
//!
//! Its own binary rather than something the services do at boot: two replicas
//! starting together would race each other through the same DDL. Compose runs it
//! as a one-shot service the indexer waits on; a deployment runs it before
//! rolling pods.
//!
//! The mechanism is refinery, not a hand-rolled runner. It drives Postgres
//! natively; ClickHouse it drives through the adapter in [`clickhouse`], which is
//! the only part that had to be written.

mod clickhouse;

use std::process::ExitCode;

use migrations::Source;
use refinery_core::{AsyncMigrate, Error, Migration, Report, Runner};

/// Which database a failure was against.
///
/// **Said here rather than inherited from whatever failed underneath.** Before
/// this, the Postgres stage looked like it named itself and only did so because
/// `postgres::Error` happens to spell "Postgres" in its message, and the
/// ClickHouse stage did not name itself at all — a missing database came back as
/// refinery quoting the server and nothing else. A dependency's wording is the
/// dependency's to change; which of two databases a deploy died on is ours to
/// state.
#[derive(Debug, thiserror::Error)]
#[error("the {database} migrations could not be applied")]
struct Failed {
    database: &'static str,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl Failed {
    fn at(
        database: &'static str,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            database,
            source: source.into(),
        }
    }
}

/// Flattens the groups into the migration set refinery is handed.
///
/// The one thing here that has to know what a migration *means*, which is why
/// it did not go to `migrations` with the files.
///
/// # Errors
///
/// Propagates refinery's parse error if a label is not `V{version}__{name}` —
/// which would be a typo in the corpus, caught the first time it runs.
fn union(sources: &[Source]) -> Result<Vec<Migration>, Error> {
    sources
        .iter()
        .flat_map(|source| source.files)
        .map(|embedded| Migration::unapplied(embedded.label, embedded.sql))
        .collect()
}

/// `current_thread`, because there is no concurrency here to serve: one
/// connection per database and a strictly sequential walk through the statements.
/// A worker pool would be startup cost for a process that exits in under a second.
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The chain, not just the head: the top line says which stage failed
            // and the cause underneath is the one naming the table or the syntax.
            //
            // A cause already quoted by the line above it is skipped, because
            // the two error conventions in this tree disagree — ours name their
            // own stage and leave the rest to `source()`, `deadpool`'s
            // interpolate theirs — and a naive walk prints the bottom half
            // twice. `ops::check` reconciles them the same way, for the same
            // reason, into a single line rather than these.
            let mut said = error.to_string();
            eprintln!("migrate: {said}");

            // Qualified, because `Error` in this module is refinery's.
            let mut cause = std::error::Error::source(&error);
            while let Some(next) = cause {
                let text = next.to_string();
                if !said.contains(&text) {
                    eprintln!("  caused by: {text}");
                    said = text;
                }
                cause = next.source();
            }
            ExitCode::FAILURE
        }
    }
}

/// ClickHouse first, then Postgres, each named on the way out whether it
/// succeeded or not.
async fn run() -> Result<(), Failed> {
    let mut target = clickhouse::ClickHouse(clickhouse_client::build_client(clickhouse_config()));
    apply("clickhouse", migrations::CLICKHOUSE, &mut target).await?;

    // One connection out of the pool, which is the only way this workspace
    // connects to Postgres. A job that runs to completion wants exactly one and
    // then exits; that it comes from a pool costs nothing and means there is not
    // a second constructor to keep in step.
    let url = env("POSTGRES_URL", POSTGRES_URL);
    let pool = postgres::build_pool(&url).map_err(|error| Failed::at("postgres", error))?;
    let mut target = postgres::connection(&pool)
        .await
        .map_err(|error| Failed::at("postgres", error))?;
    apply("postgres", migrations::POSTGRES, &mut **target).await?;

    Ok(())
}

/// Applies one database's set and says which one it was if anything goes wrong.
async fn apply<T: AsyncMigrate + Send>(
    database: &'static str,
    sources: &[Source],
    target: &mut T,
) -> Result<(), Failed> {
    let set = union(sources).map_err(|error| Failed::at(database, error))?;

    let applied = Runner::new(&set)
        .run_async(target)
        .await
        .map_err(|error| Failed::at(database, error))?;

    report(database, &applied);
    Ok(())
}

fn report(database: &str, applied: &Report) {
    let names: Vec<_> = applied
        .applied_migrations()
        .iter()
        .map(ToString::to_string)
        .collect();

    if names.is_empty() {
        println!("{database}: schema already up to date");
    } else {
        println!("{database}: applied {}", names.join(", "));
    }
}

const POSTGRES_URL: &str = "postgres://postgres@localhost:5432/postgres";

/// The environment is read here and nowhere else.
///
/// The database crates take their configuration as parameters, so they can be
/// used twice in one process against two servers and can be tested without
/// setting global state. The binary is the one place that has an environment.
fn clickhouse_config() -> clickhouse_client::Config {
    clickhouse_client::Config {
        url: env("CLICKHOUSE_URL", "http://localhost:8123"),
        database: env("CLICKHOUSE_DATABASE", "default"),
        user: env("CLICKHOUSE_USER", "default"),
        // Empty is legitimate: a container started with CLICKHOUSE_SKIP_USER_SETUP
        // has no password, which is how the test and CI instances run.
        password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
    }
}

fn env(key: &str, fallback: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| fallback.to_owned())
}

#[cfg(test)]
mod tests {
    use super::union;
    use migrations::{CLICKHOUSE, POSTGRES};

    /// Every label parses as `V{version}__{name}`. A typo in the corpus fails here
    /// rather than at deploy time.
    #[test]
    fn both_unions_parse() {
        assert_eq!(union(CLICKHOUSE).unwrap().len(), 35);
        assert_eq!(union(POSTGRES).unwrap().len(), 4);
    }

    /// Versions ascend within each database, which is the order refinery applies
    /// in. Per union, never across both: `001_spoke_events` and
    /// `001_indexer_cursor` both exist, so the two together genuinely collide.
    #[test]
    fn each_union_ascends() {
        for sources in [CLICKHOUSE, POSTGRES] {
            let versions: Vec<_> = union(sources)
                .unwrap()
                .iter()
                .map(refinery_core::Migration::version)
                .collect();

            let mut expected = versions.clone();
            expected.sort_unstable();
            expected.dedup();

            assert_eq!(versions, expected, "versions must ascend and not repeat");
        }
    }
}
