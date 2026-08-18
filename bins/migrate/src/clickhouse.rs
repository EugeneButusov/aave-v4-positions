//! Teaching refinery to drive ClickHouse.
//!
//! refinery ships backends for Postgres, MySQL and SQLite. ClickHouse is not one
//! of them, but the three traits below are all it takes — and fourteen of this
//! deployment's eighteen migrations are ClickHouse, so the alternative was a
//! second hand-rolled runner beside refinery rather than none.

use async_trait::async_trait;
use refinery_core::Migration;
use refinery_core::traits::r#async::{AsyncMigrate, AsyncQuery, AsyncTransaction};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::statements::split_statements;

/// A ClickHouse client, in the shape refinery drives.
pub struct ClickHouse(pub clickhouse::Client);

#[async_trait]
impl AsyncTransaction for ClickHouse {
    type Error = clickhouse::error::Error;

    /// **The reason this crate still owns a statement splitter.** refinery hands
    /// a migration's whole SQL as a single query, and ClickHouse's HTTP interface
    /// refuses multi-statement bodies — `Multi-statements are not allowed`. So
    /// each query is taken apart here before it is sent.
    ///
    /// There is no transaction, because ClickHouse has none for DDL. A file that
    /// fails on its third statement leaves the first two behind and never reaches
    /// refinery's ledger insert, so the retry runs it from the top — which is why
    /// every statement in this repository is written `IF NOT EXISTS`.
    async fn execute<'a, T: Iterator<Item = &'a str> + Send>(
        &mut self,
        queries: T,
    ) -> Result<usize, Self::Error> {
        let mut count = 0;
        for query in queries {
            for statement in split_statements(query) {
                self.0.query(statement).execute().await?;
                count += 1;
            }
        }

        Ok(count)
    }
}

#[async_trait]
impl AsyncQuery<Vec<Migration>> for ClickHouse {
    async fn query(&mut self, query: &str) -> Result<Vec<Migration>, Self::Error> {
        let rows = self.0.query(query).fetch_all::<Row>().await?;

        Ok(rows
            .into_iter()
            .map(|(version, name, applied_on, checksum)| {
                Migration::applied(
                    version,
                    name,
                    // Stored as text because that is what refinery's own backends
                    // do, and what its `applied_on` parser expects back.
                    OffsetDateTime::parse(&applied_on, &Rfc3339)
                        .unwrap_or(OffsetDateTime::UNIX_EPOCH),
                    checksum.parse().unwrap_or_default(),
                )
            })
            .collect())
    }
}

/// `version, name, applied_on, checksum` — refinery's ledger, in its own order.
type Row = (i32, String, String, String);

impl AsyncMigrate for ClickHouse {
    /// refinery's default is `VARCHAR(255)` columns and an `int4 PRIMARY KEY`,
    /// none of which ClickHouse has, and its DDL needs an engine and a sort key.
    /// Only the shape of the table is ours; every query over it is refinery's.
    fn assert_migrations_table_query(table: &str) -> String {
        format!(
            "CREATE TABLE IF NOT EXISTS {table} (\
               version Int32, name String, applied_on String, checksum String\
             ) ENGINE = MergeTree ORDER BY version"
        )
    }
}

#[cfg(test)]
mod tests {
    //! A real server, not a fake. What these assert is that the adapter drives
    //! ClickHouse correctly — that a multi-statement file is taken apart, that a
    //! re-run applies nothing, and that a failure part-way leaves what already
    //! succeeded, which is the property Postgres does not have.

    use clickhouse_client::{Config, client};
    use refinery_core::Runner;

    use super::*;

    async fn db(test: &str) -> ClickHouse {
        let database = format!("rust_migrate_{test}");
        let config = Config {
            url: std::env::var("CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".to_owned()),
            database: "default".to_owned(),
            user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
            password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
        };

        let admin = client(&config);
        for statement in [
            format!("DROP DATABASE IF EXISTS {database}"),
            format!("CREATE DATABASE {database}"),
        ] {
            admin.query(&statement).execute().await.unwrap();
        }

        ClickHouse(client(&Config { database, ..config }))
    }

    async fn run(target: &mut ClickHouse, migrations: &[Migration]) -> Result<Vec<String>, String> {
        Runner::new(migrations)
            .run_async(target)
            .await
            .map(|report| {
                report
                    .applied_migrations()
                    .iter()
                    .map(ToString::to_string)
                    .collect()
            })
            .map_err(|error| error.to_string())
    }

    async fn tables(target: &ClickHouse) -> Vec<String> {
        let mut names = target
            .0
            .query("SELECT name FROM system.tables WHERE database = currentDatabase()")
            .fetch_all::<String>()
            .await
            .unwrap();
        names.sort();
        names
    }

    fn at(name: &str, sql: &str) -> Migration {
        Migration::unapplied(name, sql).unwrap()
    }

    const WIDGETS: &str =
        "CREATE TABLE IF NOT EXISTS widgets (id String) ENGINE = MergeTree ORDER BY id;";
    const GADGETS: &str =
        "CREATE TABLE IF NOT EXISTS gadgets (id String) ENGINE = MergeTree ORDER BY id;";

    #[tokio::test]
    async fn creates_the_ledger_even_when_there_is_nothing_to_apply() {
        let mut target = db("empty").await;

        assert_eq!(run(&mut target, &[]).await.unwrap(), Vec::<String>::new());

        // The table refinery records into, created by our own DDL because its
        // default is Postgres-shaped.
        assert_eq!(tables(&target).await, ["refinery_schema_history"]);
    }

    #[tokio::test]
    async fn applies_each_migration_in_order_and_records_it() {
        let mut target = db("in_order").await;
        let set = [at("V1__widgets", WIDGETS), at("V2__gadgets", GADGETS)];

        assert_eq!(
            run(&mut target, &set).await.unwrap(),
            ["V1__widgets", "V2__gadgets"]
        );
        assert_eq!(
            tables(&target).await,
            ["gadgets", "refinery_schema_history", "widgets"]
        );
    }

    #[tokio::test]
    async fn applies_nothing_on_a_second_run() {
        let mut target = db("second_run").await;
        let set = [at("V1__widgets", WIDGETS), at("V2__gadgets", GADGETS)];

        run(&mut target, &set).await.unwrap();

        assert_eq!(run(&mut target, &set).await.unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn takes_a_multi_statement_file_apart() {
        let mut target = db("multi_statement").await;
        // The case the adapter exists for: refinery passes this whole string as
        // one query, and ClickHouse would reject it as a multi-statement body.
        let both = at(
            "V1__widgets",
            "CREATE TABLE IF NOT EXISTS widgets (id String) ENGINE = MergeTree ORDER BY id;\n\
             CREATE TABLE IF NOT EXISTS gadgets (id String) ENGINE = MergeTree ORDER BY id;",
        );

        assert_eq!(run(&mut target, &[both]).await.unwrap(), ["V1__widgets"]);
        assert_eq!(
            tables(&target).await,
            ["gadgets", "refinery_schema_history", "widgets"]
        );
    }

    #[tokio::test]
    async fn keeps_what_already_succeeded_when_a_later_statement_fails() {
        let mut target = db("partial").await;
        let half_broken = at(
            "V1__widgets",
            "CREATE TABLE IF NOT EXISTS widgets (id String) ENGINE = MergeTree ORDER BY id;\n\
             CREATE TABLE IF NOT EXISTS gadgets (id String ENGINE = ;",
        );

        run(&mut target, &[half_broken]).await.unwrap_err();

        // The whole difference from Postgres: no transaction for DDL, so `widgets`
        // stays. Nothing was recorded, so the retry runs the file from its first
        // statement — which is why every statement is `IF NOT EXISTS`.
        assert_eq!(
            tables(&target).await,
            ["refinery_schema_history", "widgets"]
        );
    }
}
