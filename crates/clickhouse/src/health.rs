use crate::error::Error;

/// Reports ClickHouse able to serve a query.
///
/// Schema integrity belongs to the migration gate and data integrity to
/// reconciliation. Readiness asks only whether this dependency is available
/// now, and keeps the driver's potentially sensitive response behind [`Error`].
///
/// # Errors
///
/// [`Error::Unreachable`] if the query cannot be run.
pub async fn ping(client: &clickhouse::Client) -> Result<(), Error> {
    client
        .query("SELECT 1")
        .fetch_one::<u8>()
        .await
        .map(drop)
        .map_err(|source| Error::Unreachable { source })
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::net::TcpListener;
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
    async fn answers_when_the_server_can_serve_a_query() {
        ping(&crate::build_client(config("default")))
            .await
            .expect("reachable");
    }

    /// Without a TLS feature the driver refuses the scheme before it dials:
    /// `hyper-util`'s connect error is turned into `Unsupported("invalid URL,
    /// scheme is not http; … consider enabling `native-tls` or `rustls-tls`")`,
    /// and that branch is compiled out once one of them is on.
    #[tokio::test]
    async fn dials_an_https_url_rather_than_refusing_the_scheme() {
        let client = crate::build_client(crate::Config {
            url: "https://127.0.0.1:1".to_owned(),
            ..config("default")
        });

        let refused = ping(&client).await.expect_err("port 1 is reserved");
        let cause = std::error::Error::source(&refused)
            .expect("the cause is kept for the log")
            .to_string();

        assert!(!cause.contains("scheme is not http"), "{cause}");
        assert!(!cause.contains("rustls-tls"), "{cause}");
    }

    #[tokio::test]
    async fn uses_plain_http_when_the_url_asks_for_it() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut opening = [0; 5];
            stream.read_exact(&mut opening).unwrap();
            opening
        });
        let client = crate::build_client(crate::Config {
            url: format!("http://{address}"),
            ..config("default")
        });

        let _ = ping(&client).await;

        assert_eq!(server.join().unwrap(), *b"POST ");
    }

    /// The driver quotes the server's response: an authentication failure, a
    /// table name, a version string. `/health/ready` is unauthenticated.
    #[tokio::test]
    async fn keeps_the_servers_own_words_out_of_the_message() {
        let client = crate::build_client(crate::Config {
            password: "not-the-password".to_owned(),
            ..config("default")
        });

        let refused = ping(&client).await.expect_err("a wrong password");

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
