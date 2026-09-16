//! What a statement looks like in a trace.
//!
//! **Here rather than at each store**, for the reason the probe paths live in
//! `ops`: three crates issue one query each, and three spellings of the same
//! span is three things a dashboard has to know about.
//!
//! The driver emits nothing of its own — `tokio-postgres` speaks `log`, not
//! `tracing`, and opens no span at all — so this is the whole of what a Postgres
//! read looks like from outside.

/// A client span for one statement, named as the specification names them.
///
/// **`sql` is published as `db.query.text`, so it must be a constant.** Every
/// caller here passes the `const` it also passes to the driver, and every value
/// travels as a bind parameter — which is what makes publishing the text safe.
/// A formatted string would put whatever was interpolated into it on a span
/// anyone with the trace can read.
#[must_use]
pub fn query_span(
    operation: &'static str,
    table: &'static str,
    sql: &'static str,
) -> tracing::Span {
    tracing::info_span!(
        "postgres.query",
        otel.name = format!("{operation} {table}"),
        otel.kind = "client",
        db.system.name = "postgresql",
        db.operation.name = operation,
        db.collection.name = table,
        db.query.text = sql,
    )
}
