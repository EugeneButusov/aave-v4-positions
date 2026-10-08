//! A connection whose reads appear in a trace.
//!
//! **The span is not something a call site can leave off**, which is the whole
//! reason this type exists rather than a `Span` to hang beside `client.query`.
//! The driver opens none of its own — `tokio-postgres` speaks `log`, not
//! `tracing` — so a read issued straight against it is a gap in every trace that
//! passes through, and nothing anywhere fails.
//!
//! Here rather than at each store for the reason the probe paths live in `ops`:
//! three crates read one statement each, and three spellings of the same span is
//! three things a dashboard has to know about.

use tokio_postgres::Row;
use tokio_postgres::types::ToSql;

use crate::error::Error;
use crate::pool::{Pool, connection};

/// A pool whose reads are seen, and which does nothing else.
///
/// **Held rather than acquired**, which is the whole shape: a store takes one of
/// these once in its constructor, and a read is a single line with no connection
/// bookkeeping in front of it. The connection is still taken per read, because
/// it has to be — `deadpool` checks `is_closed()` when handing one out, so a
/// connection held across a server restart or an idle timeout is one that fails
/// every request from then on while the readiness probe, which asks the pool for
/// a fresh one, still reports healthy. Measured, that check costs **1.4 µs**
/// against **552 µs** for the `SELECT` it precedes.
///
/// **A wrapper rather than an extension trait**, which is what makes the method
/// names the driver's own: `query` here is this one, and the untraced `query`
/// underneath is not reachable through it at all. An extension trait had to call
/// its methods something else, because an inherent method wins resolution.
///
/// [`Client`](crate::Client) stays an alias: refinery implements its traits for
/// that exact type, and `bins/migrate` hands it `&mut **connection`.
pub struct Traced(Pool);

impl Traced {
    #[must_use]
    pub fn new(pool: Pool) -> Self {
        Self(pool)
    }

    /// **`name` is `{operation} {target}`** — the shape the specification spells
    /// for a database span name — and it is given rather than derived, because
    /// deriving it means parsing SQL. `traced-sql.ts` did that, and produced a
    /// span called `SELECT its` out of prose inside a comment.
    ///
    /// **`sql` is `&'static str` because it is published as `db.query.text`.**
    /// Every value travels as a bind parameter, which is what makes publishing
    /// the text safe, and the type is what keeps that true rather than a note
    /// asking the next caller not to pass a `format!`.
    ///
    /// # Errors
    ///
    /// Whatever the driver says about the statement or the connection under it.
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

    /// At most one row, as the driver's `query_opt` means it.
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

/// **`db.operation.name` and `db.collection.name` are deliberately absent.**
/// The specification asks for them when they are readily available, and here
/// they would be — but nothing in this deployment reads either, and the
/// `clickhouse` driver beside this emits neither for the same span. The name is
/// what a person reads in a trace list, and it is the part that earns its keep.
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

    /// Somewhere a case can read a span back from. `MakeWriter` is implemented
    /// for any `Fn() -> impl Write`, so this needs no impl of its own.
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

    /// Makes every callsite in this binary interesting, once.
    ///
    /// **`tracing` caches callsite interest globally**, and computes it from the
    /// *global* subscriber — `NoSubscriber` here, which answers "never" for every
    /// callsite. A case that reads a span back through a scoped subscriber then
    /// depends on whether its callsite was first registered inside one, and a
    /// sibling case issuing a query with nothing in scope is enough to leave it
    /// disabled for the rest of the process. Measured, not theorised: the run
    /// that caught this failed this case and the
    /// ClickHouse one together, each with an empty buffer.
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

        // The name and the statement are both `&'static str` and adjacent, so
        // this is also what fails if a caller passes them the other way round.
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
        // Safe to publish the text only because the value above travelled as
        // `$1` rather than through the string.
        assert!(
            !written.contains("public"),
            "a bound value reached the span"
        );
    }
}
