/// Reports the fold reachable.
///
/// A plain `SELECT 1` rather than anything about row counts or lag: this
/// answers "can this process read at all", and a store it cannot reach is a
/// readiness problem. Whether the *data* is right is reconciliation's question
/// (§9), and it needs its own signal rather than being folded in here.
///
/// Here rather than in the binary that serves the probe, because both of them
/// want it — the API reads the fold and the indexer writes it — and a check
/// written in one process is a check the other copies.
///
/// # Errors
///
/// Whatever the driver says, which for an unreachable server is the connection
/// error and is the half of the message worth reading.
pub async fn ping(client: &clickhouse::Client) -> Result<(), clickhouse::error::Error> {
    client.query("SELECT 1").fetch_one::<u8>().await.map(drop)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::instrument::WithSubscriber as _;
    use tracing_subscriber::fmt::format::FmtSpan;

    use super::*;

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

    /// What the driver's `opentelemetry` feature buys, as a query can show it.
    ///
    /// **The span is emitted either way**; the feature is what makes it a client
    /// span rather than an unattributed one, and what puts `traceparent` on the
    /// request so the server can be asked about the same trace. Dropping the
    /// feature from the manifest is silent everywhere else.
    #[tokio::test]
    async fn a_query_is_a_client_span_of_a_named_system() {
        let client = crate::build_client(crate::Config {
            url: std::env::var("CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".to_owned()),
            database: "default".to_owned(),
            user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
            password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
        });
        let sink = Sink::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer({
                let sink = sink.clone();
                move || sink.clone()
            })
            .with_ansi(false)
            .with_span_events(FmtSpan::CLOSE)
            .finish();

        ping(&client).with_subscriber(subscriber).await.unwrap();

        let written = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();

        assert!(written.contains("clickhouse.query"), "{written}");
        assert!(
            written.contains(r#"db.system.name="clickhouse""#),
            "{written}"
        );
        assert!(written.contains(r#"otel.kind="client""#), "{written}");
    }
}
