/// Reports the fold reachable.
///
/// A plain `SELECT 1`: this answers "can this process read at all", and
/// whether the data is right is reconciliation's question (§9).
///
/// Here rather than in the binary serving the probe, because both the API and
/// the indexer want it.
///
/// # Errors
///
/// Whatever the driver says, which for an unreachable server is the connection
/// error.
pub async fn ping(client: &clickhouse::Client) -> Result<(), clickhouse::error::Error> {
    client.query("SELECT 1").fetch_one::<u8>().await.map(drop)
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

    #[tokio::test]
    async fn a_query_is_a_client_span_of_a_named_system() {
        interesting();
        let client = crate::build_client(crate::Config {
            url: std::env::var("CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".to_owned()),
            database: "default".to_owned(),
            user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
            password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
        });
        let (captured, lines) = Captured::new();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured)
            .with_ansi(false)
            .with_span_events(FmtSpan::CLOSE)
            .finish();

        ping(&client).with_subscriber(subscriber).await.unwrap();

        let written = lines.read();

        assert!(written.contains("clickhouse.query"), "{written}");
        assert!(
            written.contains(r#"db.system.name="clickhouse""#),
            "{written}"
        );
        assert!(written.contains(r#"otel.kind="client""#), "{written}");
    }
}
