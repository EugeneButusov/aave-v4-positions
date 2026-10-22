use crate::error::Error;

/// The relations a caller named that this database does not have.
///
/// `CAST` because an empty literal is `Array(Nothing)`, which the server refuses
/// as a filter rather than reading as no rows.
const MISSING: &str = "SELECT arrayJoin(CAST(? AS Array(String))) AS want \
     WHERE want NOT IN (SELECT name FROM system.tables WHERE database = currentDatabase())";

/// Reports the fold reachable, and holding the relations a caller reads.
///
/// `SELECT 1` answered the first half alone, so a database that had never been
/// migrated reported itself ready and then served 500s. A parameterised view is
/// a row in `system.tables` like any table, which is what lets one query cover
/// both. Whether the rows are *right* is reconciliation's question (§9).
///
/// # Errors
///
/// [`Error::Unreachable`] if the query cannot be run at all,
/// [`Error::Missing`] naming every relation this database does not have.
pub async fn ping(client: &clickhouse::Client, relations: &[&str]) -> Result<(), Error> {
    let missing = client
        .query(MISSING)
        .bind(relations)
        .fetch_all::<String>()
        .await
        .map_err(|source| Error::Unreachable { source })?;

    if missing.is_empty() {
        return Ok(());
    }

    Err(Error::Missing {
        missing: missing.join(", "),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use tracing::instrument::WithSubscriber as _;
    use tracing_subscriber::fmt::format::FmtSpan;
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;

    /// Somewhere a case can read a subscriber's output back from.
    ///
    /// A channel rather than a shared buffer: a `Sender` is `Clone` and
    /// synchronised already, so this needs neither an `Arc` nor a `Mutex`.
    #[derive(Clone)]
    struct Captured(mpsc::Sender<Vec<u8>>);

    impl Captured {
        fn new() -> (Self, Lines) {
            let (sender, received) = mpsc::channel();

            (Self(sender), Lines(received))
        }
    }

    impl std::io::Write for Captured {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.send(buffer.to_vec());

            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// The other end. [`Lines::read`] consumes it, because a channel is drained
    /// rather than re-read.
    struct Lines(mpsc::Receiver<Vec<u8>>);

    impl Lines {
        fn read(self) -> String {
            self.0
                .try_iter()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .collect()
        }
    }

    /// What the driver's `opentelemetry` feature buys: the span is emitted
    /// either way, the feature makes it a client span and adds `traceparent`.
    /// Dropping it from the manifest is silent everywhere else.
    /// `tracing` caches callsite interest from the *global* subscriber, which is
    /// `NoSubscriber` here and answers "never". A sibling case querying with
    /// nothing in scope leaves `clickhouse.query` disabled for the process.
    fn interesting() {
        static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();

        ONCE.get_or_init(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::registry().with(tracing::level_filters::LevelFilter::TRACE),
            );
        });
    }

    /// One database per case: `cargo` runs these concurrently against one
    /// server, and both of them create the same table.
    async fn with_one_table(name: &str) -> clickhouse::Client {
        let admin = crate::build_client(config("default"));
        for statement in [
            format!("DROP DATABASE IF EXISTS {name} SYNC"),
            format!("CREATE DATABASE {name}"),
        ] {
            admin.query(&statement).execute().await.unwrap();
        }

        let client = crate::build_client(config(name));
        client
            .query("CREATE TABLE widgets (id String) ENGINE = MergeTree ORDER BY id")
            .execute()
            .await
            .unwrap();
        client
    }

    fn config(database: &str) -> crate::Config {
        crate::Config {
            url: std::env::var("CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".to_owned()),
            database: database.to_owned(),
            user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
            password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
            max_execution: std::time::Duration::from_secs(30),
        }
    }

    #[tokio::test]
    async fn takes_a_database_holding_every_relation_it_was_asked_for() {
        let client = with_one_table("rust_health_present").await;

        ping(&client, &["widgets"]).await.expect("just created");
    }

    #[tokio::test]
    async fn names_the_relation_the_migrations_did_not_build() {
        let client = with_one_table("rust_health_missing").await;

        let refused = ping(&client, &["widgets", "user_positions_as_of"])
            .await
            .expect_err("one of the two is not there");

        assert_eq!(
            refused.to_string(),
            "the fold is missing user_positions_as_of"
        );
    }

    #[tokio::test]
    async fn asking_for_nothing_still_proves_the_server_answers() {
        ping(&crate::build_client(config("default")), &[])
            .await
            .expect("an empty list is no relations, not a bad query");
    }

    /// The driver quotes the server's response: an authentication failure, a
    /// table name, a version string. `/health/ready` is unauthenticated.
    #[tokio::test]
    async fn keeps_the_servers_own_words_out_of_the_message() {
        let client = crate::build_client(crate::Config {
            password: "not-the-password".to_owned(),
            ..config("default")
        });

        let refused = ping(&client, &["widgets"])
            .await
            .expect_err("a wrong password");

        assert_eq!(refused.to_string(), "could not reach ClickHouse");
        assert!(
            std::error::Error::source(&refused)
                .expect("the cause is kept for the log")
                .to_string()
                .contains("Authentication failed")
        );
    }

    #[tokio::test]
    async fn a_query_is_a_client_span_of_a_named_system() {
        interesting();
        let client = crate::build_client(config("default"));
        let (captured, lines) = Captured::new();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured)
            .with_ansi(false)
            .with_span_events(FmtSpan::CLOSE)
            .finish();

        ping(&client, &[])
            .with_subscriber(subscriber)
            .await
            .unwrap();

        let written = lines.read();

        assert!(written.contains("clickhouse.query"), "{written}");
        assert!(
            written.contains(r#"db.system.name="clickhouse""#),
            "{written}"
        );
        assert!(written.contains(r#"otel.kind="client""#), "{written}");
    }
}
