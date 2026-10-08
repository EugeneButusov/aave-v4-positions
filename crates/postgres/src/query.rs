//! Reads that carry a span, because the driver opens none: `tokio-postgres`
//! speaks `log`, not `tracing`.

use tokio_postgres::Row;
use tokio_postgres::types::ToSql;

use crate::error::Error;
use crate::pool::{Pool, connection};

/// A pool whose reads are seen.
///
/// **The connection is taken per read, never held.** `deadpool` checks
/// `is_closed()` when handing one out, so a held connection fails every request
/// after a server restart while readiness, asking for a fresh one, still passes.
pub struct Traced(Pool);

impl Traced {
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self(pool)
    }

    /// `name` is `{operation} {target}`, given rather than parsed out of `sql`:
    /// parsing it produced a span called `SELECT its` from prose in a comment.
    ///
    /// # Errors
    ///
    /// No connection to be had, or one the server refused.
    pub async fn query(
        &self,
        name: &'static str,
        sql: &'static str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, Error> {
        use tracing::Instrument as _;

        connection(&self.0)
            .await?
            .query(sql, params)
            .instrument(span(name, sql))
            .await
            .map_err(|source| Error::QueryFailed { source })
    }

    /// At most one row.
    ///
    /// # Errors
    ///
    /// As [`query`](Self::query).
    pub async fn query_opt(
        &self,
        name: &'static str,
        sql: &'static str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, Error> {
        use tracing::Instrument as _;

        connection(&self.0)
            .await?
            .query_opt(sql, params)
            .instrument(span(name, sql))
            .await
            .map_err(|source| Error::QueryFailed { source })
    }
}

fn span(name: &'static str, sql: &'static str) -> tracing::Span {
    tracing::info_span!(
        "postgres.query",
        otel.name = name,
        otel.kind = "client",
        db.system.name = "postgresql",
        db.query.text = sql,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::instrument::WithSubscriber as _;
    use tracing_subscriber::fmt::format::FmtSpan;
    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;
    use crate::build_pool;

    /// A statement every server has the answer to, so this needs no schema.
    const TABLES: &str =
        "SELECT schemaname FROM pg_catalog.pg_tables WHERE schemaname = $1 LIMIT 1";

    /// Somewhere a case can read a span back from.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Sink {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// `tracing` caches callsite interest from the *global* subscriber, which is
    /// `NoSubscriber` here and answers "never". Without this, whether a scoped
    /// subscriber sees a span depends on which case ran first.
    fn interesting() {
        static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();

        ONCE.get_or_init(|| {
            let _ = tracing::subscriber::set_global_default(
                tracing_subscriber::registry().with(tracing::level_filters::LevelFilter::TRACE),
            );
        });
    }

    #[tokio::test]
    async fn a_read_is_one_client_span_naming_what_it_read() {
        interesting();
        let url = std::env::var("POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned());
        let pool = build_pool(&url).unwrap();
        let client = Traced::new(pool);
        let sink = Sink::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer({
                let sink = sink.clone();
                move || sink.clone()
            })
            .with_ansi(false)
            .with_span_events(FmtSpan::CLOSE)
            .finish();

        client
            .query("SELECT pg_tables", TABLES, &[&"public"])
            .with_subscriber(subscriber)
            .await
            .unwrap();

        let written = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();

        assert!(
            written.contains(r#"otel.name="SELECT pg_tables""#),
            "{written}"
        );
        assert!(written.contains(r#"otel.kind="client""#), "{written}");
        assert!(
            written.contains(r#"db.system.name="postgresql""#),
            "{written}"
        );
        assert!(written.contains("WHERE schemaname = $1"), "{written}");
        assert!(
            !written.contains("public"),
            "a bound value reached the span"
        );
    }
}
