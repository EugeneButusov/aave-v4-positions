//! Reads that appear in a trace.
//!
//! **The span is not something a call site can leave off**, which is why a read
//! goes through here at all. The driver opens none of its own — `tokio-postgres`
//! speaks `log`, not `tracing` — so a read issued straight against it is a gap
//! in every trace that passes through, and nothing anywhere fails.
//!
//! Here rather than at each store for the reason the probe paths live in `ops`:
//! three crates read one statement each, and three spellings of the same span is
//! three things a dashboard has to know about.

use std::future::Future;

use tokio_postgres::types::ToSql;
use tokio_postgres::{Error, Row};

use crate::Client;

/// Reads that carry a span, on the driver's own client.
///
/// **A trait because [`Client`] is an alias for the driver's type.** An inherent
/// `impl` on it is `E0116` — rustc's words, "does not define a new type, only an
/// alias" — and a newtype is what [`crate`] already rules out, refinery
/// implementing its traits for that exact type. Reached through
/// [`Connection`](crate::Connection)'s `Deref`, which is what every store holds.
///
/// **The methods cannot be called `query`.** Inherent methods win resolution, so
/// `client.query(name, sql, params)` would find the driver's two-argument one
/// and fail on arity rather than arriving here. The suffix is what the method
/// adds, which also puts the difference at the call site: `query_traced` beside
/// `query` says which one a reader is looking at without going to the imports.
///
/// **`name` is `{operation} {target}`** — the shape the specification spells for
/// a database span name — and it is given rather than derived, because deriving
/// it means parsing SQL. `traced-sql.ts` did that, and produced a span called
/// `SELECT its` out of prose inside a comment.
///
/// **Desugared rather than `async fn`**, because an `async fn` in a public trait
/// leaves the future's auto traits unstated: `Send` is what lets a caller spawn
/// a read, and in a trait that cannot be added later without breaking the API.
///
/// **`sql` is `&'static str` because it is published as `db.query.text`.** Every
/// value travels as a bind parameter, which is what makes publishing the text
/// safe, and the type is what keeps that true rather than a note asking the next
/// caller not to pass a `format!`.
pub trait Traced {
    /// # Errors
    ///
    /// Whatever the driver says about the statement or the connection under it.
    fn query_traced(
        &self,
        name: &'static str,
        sql: &'static str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Vec<Row>, Error>> + Send;

    /// At most one row, as the driver's `query_opt` means it.
    ///
    /// # Errors
    ///
    /// As [`query_traced`](Self::query_traced).
    fn query_opt_traced(
        &self,
        name: &'static str,
        sql: &'static str,
        params: &[&(dyn ToSql + Sync)],
    ) -> impl Future<Output = Result<Option<Row>, Error>> + Send;
}

impl Traced for Client {
    async fn query_traced(
        &self,
        name: &'static str,
        sql: &'static str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Vec<Row>, Error> {
        use tracing::Instrument as _;

        self.query(sql, params).instrument(span(name, sql)).await
    }

    async fn query_opt_traced(
        &self,
        name: &'static str,
        sql: &'static str,
        params: &[&(dyn ToSql + Sync)],
    ) -> Result<Option<Row>, Error> {
        use tracing::Instrument as _;

        self.query_opt(sql, params)
            .instrument(span(name, sql))
            .await
    }
}

/// **`db.operation.name` and `db.collection.name` are deliberately absent.**
/// The specification asks for them when they are readily available, and here
/// they would be — but nothing in this deployment reads either, and the
/// `clickhouse` driver beside this emits neither for the same span. The name
/// below is what a person reads in a trace list, and it is the part that earns
/// its keep.
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

    use super::*;
    use crate::{build_pool, connection};

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

    #[tokio::test]
    async fn a_traced_query_is_one_client_span_naming_what_it_read() {
        let url = std::env::var("POSTGRES_URL")
            .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned());
        let pool = build_pool(&url).unwrap();
        let client = connection(&pool).await.unwrap();
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
            .query_traced("SELECT pg_tables", TABLES, &[&"public"])
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
