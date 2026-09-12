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
    pub database: String,
    pub user: String,
    /// Empty is legitimate: a container started with `CLICKHOUSE_SKIP_USER_SETUP`
    /// has no password, which is how the test and CI instances run.
    pub password: String,
}

/// The client every caller in this workspace should use.
///
/// Takes the config by value, because the builder keeps what it is handed: its
/// `impl Into<String>` moves an owned `String` and allocates a fresh one from a
/// borrow. A caller that wants to keep its config clones visibly, rather than
/// every caller paying for a copy it cannot see.
#[must_use]
pub fn build_client(config: Config) -> clickhouse::Client {
    clickhouse::Client::default()
        .with_url(config.url)
        .with_database(config.database)
        .with_user(config.user)
        .with_password(config.password)
}

#[cfg(test)]
mod tests {
    //! That every field of a [`Config`] reaches the client.
    //!
    //! Worth a test because nothing else would notice. This builder cannot fail
    //! — there is no connection to refuse it — so a field dropped or two of them
    //! swapped produces a working client pointed somewhere else, and every other
    //! test in this workspace runs as `default` against `default` with no
    //! password, where wrong and right look identical.

    use super::*;

    fn config(database: &str) -> Config {
        Config {
            url: std::env::var("CLICKHOUSE_URL")
                .unwrap_or_else(|_| "http://localhost:8123".to_owned()),
            database: database.to_owned(),
            user: std::env::var("CLICKHOUSE_USER").unwrap_or_else(|_| "default".to_owned()),
            password: std::env::var("CLICKHOUSE_PASSWORD").unwrap_or_default(),
        }
    }

    async fn scalar(client: &clickhouse::Client, query: &str) -> String {
        client.query(query).fetch_one::<String>().await.unwrap()
    }

    #[tokio::test]
    async fn the_database_and_user_are_the_ones_asked_for() {
        let database = "rust_client_wiring";
        let admin = build_client(config("default"));
        admin
            .query(&format!("CREATE DATABASE IF NOT EXISTS {database}"))
            .execute()
            .await
            .unwrap();

        let client = build_client(config(database));

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
        });

        let refused = client
            .query("SELECT 1")
            .fetch_one::<u8>()
            .await
            .unwrap_err()
            .to_string();

        assert!(refused.contains("Authentication failed"), "{refused}");
    }
}
