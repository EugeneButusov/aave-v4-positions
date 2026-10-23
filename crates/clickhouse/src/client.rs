use std::time::Duration;

use crate::Error;
use hyper_util::{
    client::legacy::{Client as HyperClient, connect::HttpConnector},
    rt::TokioExecutor,
};

/// Whether this process may send credentials and queries without TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportSecurity {
    RequireTls,
    /// For an explicitly isolated development or test network only.
    AllowInsecureHttp,
}

/// How to reach ClickHouse.
///
/// Four discrete parts rather than one URL, because that is what the server's
/// HTTP interface takes and what every deployment of it hands you. Postgres gets
/// a URL for the opposite reason — see `postgres::build_pool`.
///
/// Taken as parameters rather than read from the environment: a library that
/// reaches for `std::env` cannot be used twice in one process with two different
/// configurations, and cannot be tested without setting global state. The binary
/// parses the environment and passes the result down.
#[derive(Debug, Clone)]
pub struct Config {
    pub url: String,
    pub transport_security: TransportSecurity,
    pub database: String,
    pub user: String,
    /// Empty is legitimate: a container started with `CLICKHOUSE_SKIP_USER_SETUP`
    /// has no password, which is how the test and CI instances run.
    pub password: String,

    /// `max_execution_time`. Without it the HTTP connection is the only thing
    /// that ends a pathological query.
    pub max_execution: Duration,
}

/// The client every caller in this workspace should use.
///
/// Takes the config by value, because the builder keeps what it is handed: its
/// `impl Into<String>` moves an owned `String` and allocates a fresh one from a
/// borrow. A caller that wants to keep its config clones visibly, rather than
/// every caller paying for a copy it cannot see.
///
/// # Errors
///
/// [`Error::BadUrl`] when the endpoint is not a URL,
/// [`Error::InsecureTransport`] when it is HTTP without an explicit exception,
/// and [`Error::UnsupportedScheme`] for anything other than HTTP or HTTPS.
pub fn build_client(config: Config) -> Result<clickhouse::Client, Error> {
    let endpoint = url::Url::parse(&config.url).map_err(|source| Error::BadUrl { source })?;

    let client = match endpoint.scheme() {
        "https" => clickhouse::Client::default(),
        "http" if config.transport_security == TransportSecurity::AllowInsecureHttp => {
            // Avoid loading certificate roots for the one mode that promises
            // not to use them. The driver's TLS-enabled default creates its
            // root store eagerly even for an HTTP URL.
            let mut connector = HttpConnector::new();
            connector.set_keepalive(Some(Duration::from_secs(60)));
            connector.enforce_http(true);
            let client = HyperClient::builder(TokioExecutor::new())
                .pool_idle_timeout(Duration::from_secs(2))
                .build(connector);
            clickhouse::Client::with_http_client(client)
        }
        "http" => return Err(Error::InsecureTransport),
        scheme => {
            return Err(Error::UnsupportedScheme {
                scheme: scheme.to_owned(),
            });
        }
    };

    Ok(client
        .with_url(config.url)
        .with_database(config.database)
        .with_user(config.user)
        .with_password(config.password)
        .with_setting(
            "max_execution_time",
            max_execution_seconds(config.max_execution).to_string(),
        ))
}

/// At least one: `max_execution_time=0` is this server's "no limit", so a
/// sub-second deadline would remove the one it was asked to set.
fn max_execution_seconds(limit: Duration) -> u64 {
    limit.as_secs().max(1)
}

#[cfg(test)]
mod tests {
    //! That every field of a [`Config`] reaches the client, and that its
    //! transport policy is decided before the first request.
    //!
    //! Worth a test because a field dropped or two of them swapped produces a
    //! working client pointed somewhere else, and every other test in this
    //! workspace runs as `default` against `default` with no password, where
    //! wrong and right look identical.

    use std::io::Read as _;
    use std::net::TcpListener;
    use std::sync::mpsc;

    use super::*;

    fn config(database: &str) -> Config {
        Config {
            url: std::env::var("CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".to_owned()),
            transport_security: TransportSecurity::AllowInsecureHttp,
            database: database.to_owned(),
            user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
            password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
            max_execution: Duration::from_secs(30),
        }
    }

    async fn scalar(client: &clickhouse::Client, query: &str) -> String {
        client.query(query).fetch_one::<String>().await.unwrap()
    }

    #[tokio::test]
    async fn the_database_and_user_are_the_ones_asked_for() {
        let database = "rust_client_wiring";
        let admin = build_client(config("default")).unwrap();
        admin
            .query(&format!("CREATE DATABASE IF NOT EXISTS {database}"))
            .execute()
            .await
            .unwrap();

        let client = build_client(config(database)).unwrap();

        // Asked of the server rather than read back off the builder: what
        // matters is what arrived, not what was set.
        assert_eq!(scalar(&client, "SELECT currentDatabase()").await, database);
        assert_eq!(
            scalar(&client, "SELECT currentUser()").await,
            config("default").user
        );
    }

    /// The field with no positive evidence anywhere else: a client that dropped
    /// the password still works against a server that does not require one, so
    /// the only proof it is sent is a wrong one being refused.
    #[tokio::test]
    async fn the_password_is_sent() {
        let client = build_client(Config {
            password: "not-the-password".to_owned(),
            ..config("default")
        })
        .unwrap();

        let refused = client
            .query("SELECT 1")
            .fetch_one::<u8>()
            .await
            .unwrap_err()
            .to_string();

        assert!(refused.contains("Authentication failed"), "{refused}");
    }

    #[test]
    fn refuses_http_unless_the_caller_names_the_exception() {
        let error = build_client(Config {
            transport_security: TransportSecurity::RequireTls,
            ..config("default")
        })
        .err()
        .expect("plain HTTP must be refused");

        assert!(matches!(error, Error::InsecureTransport));
    }

    #[tokio::test]
    async fn https_opens_with_a_tls_handshake() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sent, received) = mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut opening = [0];
            stream.read_exact(&mut opening).unwrap();
            let _ = sent.send(opening[0]);
        });
        let client = build_client(Config {
            url: format!("https://{address}"),
            transport_security: TransportSecurity::RequireTls,
            ..config("default")
        })
        .unwrap();

        let _ = client.query("SELECT 1").fetch_one::<u8>().await;

        assert_eq!(
            received.recv_timeout(Duration::from_secs(1)).unwrap(),
            0x16,
            "a TLS handshake record"
        );
    }

    #[test]
    fn accepts_http_only_when_the_exception_is_explicit() {
        assert!(build_client(config("default")).is_ok());
    }

    #[test]
    fn refuses_an_unknown_transport_before_it_dials() {
        let error = build_client(Config {
            url: "ftp://clickhouse.example".to_owned(),
            ..config("default")
        })
        .err()
        .expect("an unknown scheme must be refused");

        assert!(matches!(error, Error::UnsupportedScheme { .. }));
    }

    #[test]
    fn refuses_a_url_that_does_not_parse() {
        let error = build_client(Config {
            url: "://".to_owned(),
            ..config("default")
        })
        .err()
        .expect("an invalid URL must be refused");

        assert!(matches!(error, Error::BadUrl { .. }));
    }
}
