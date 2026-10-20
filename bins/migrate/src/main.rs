//! Applies both schemas, then exits.
//!
//! Its own binary rather than something the services do at boot: two replicas
//! starting together would race each other through the same DDL.
//!
//! refinery drives Postgres natively; ClickHouse it drives through the adapter
//! in [`clickhouse`], which is the only part that had to be written.

mod clickhouse;

use std::process::ExitCode;

use migrations::Source;
use refinery_core::{AsyncMigrate, Error, Migration, Report, Runner};

/// What stops a run, and which database it was against — said here rather than
/// inherited from whatever failed underneath: a dependency's wording is the
/// dependency's to change, and refinery names neither database.
#[derive(Debug, thiserror::Error)]
enum Failed {
    /// Before either database is reached, so it names neither.
    #[error("the environment could not be read")]
    Configuration(#[from] env::Invalid),

    #[error("the {database} migrations could not be applied")]
    Database {
        database: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl Failed {
    fn at(
        database: &'static str,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self::Database {
            database,
            source: source.into(),
        }
    }
}

/// Flattens the groups into the migration set refinery is handed.
///
/// The one thing that has to know what a migration means, which is why it did
/// not go to `migrations` with the files.
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

/// `current_thread`: one connection per database and a sequential walk, so a
/// worker pool would be startup cost for a process that exits in a second.
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The chain, not just the head: the top line says which stage
            // failed and the cause underneath names the table or the syntax. A
            // cause already quoted above it is skipped, as
            // `ops::error_message_with_causes` does and for the same reason.
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

async fn run() -> Result<(), Failed> {
    let Settings {
        clickhouse,
        postgres,
    } = Settings::from_env()?;

    let mut target = clickhouse::ClickHouse(clickhouse_client::build_client(clickhouse));
    apply("clickhouse", migrations::CLICKHOUSE, &mut target).await?;

    // The pool is the only way this workspace connects to Postgres; one
    // connection out of it costs nothing and leaves no second constructor.
    let pool = postgres::build_pool(&postgres).map_err(|error| Failed::at("postgres", error))?;
    let mut target = postgres::connection(&pool)
        .await
        .map_err(|error| Failed::at("postgres", error))?;
    apply("postgres", migrations::POSTGRES, &mut **target).await?;

    Ok(())
}

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

/// Which two servers to migrate.
///
/// The environment is read here and nowhere else: the database crates take
/// their configuration as parameters, so a case needs no global state.
struct Settings {
    clickhouse: clickhouse_client::Config,
    postgres: String,
}

impl Settings {
    /// Through `env`, which is what `bins/api` reads with — so a `.env` that
    /// points the API at one database no longer leaves the migrator pointed at
    /// another, and a URL that will not parse is refused before any DDL runs.
    ///
    /// # Errors
    ///
    /// [`env::Invalid`], naming the first variable that could not be read.
    fn from_env() -> Result<Self, env::Invalid> {
        let vars = env::Source::from_env().read();
        let env = env::Env::new(&vars);

        Ok(Self {
            clickhouse: clickhouse_client::Config {
                url: env.url("CLICKHOUSE_URL", "http://localhost:8123")?,
                database: env.text("CLICKHOUSE_DATABASE", "default"),
                user: env.text("CLICKHOUSE_USER", "default"),
                // Empty is legitimate: `CLICKHOUSE_SKIP_USER_SETUP` leaves no
                // password, which is how the test and CI instances run.
                password: env.text("CLICKHOUSE_PASSWORD", ""),
            },
            postgres: env.url(
                "POSTGRES_URL",
                "postgres://postgres@localhost:5432/postgres",
            )?,
        })
    }
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

    /// Versions ascend within each database, which is the order refinery
    /// applies in. Per union, never across both, which genuinely collides.
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
