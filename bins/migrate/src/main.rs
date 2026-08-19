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
mod schema;

#[cfg(test)]
mod completeness;

use std::process::ExitCode;

use refinery_core::{Report, Runner};

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
            eprintln!("migrate: {error}");
            let mut cause = error.source();
            while let Some(next) = cause {
                eprintln!("  caused by: {next}");
                cause = next.source();
            }
            ExitCode::FAILURE
        }
    }
}

/// ClickHouse first, then Postgres, each reported under its own name so a failure
/// says which database it was rather than leaving that to be inferred.
async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut target = clickhouse::ClickHouse(clickhouse_client::build_client(clickhouse_config()));
    let applied = Runner::new(&schema::union(schema::CLICKHOUSE)?)
        .run_async(&mut target)
        .await?;
    report("clickhouse", &applied);

    let mut target = postgres::build_client(&env("POSTGRES_URL", POSTGRES_URL)).await?;
    let applied = Runner::new(&schema::union(schema::POSTGRES)?)
        .run_async(&mut target)
        .await?;
    report("postgres", &applied);

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
    use crate::schema::{CLICKHOUSE, POSTGRES, union};

    /// Every label parses as `V{version}__{name}`. A typo in schema.rs fails here
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
