/// How to reach ClickHouse.
///
/// Four discrete parts rather than one URL, because that is what the server's
/// HTTP interface takes and what every deployment of it hands you. Postgres gets
/// a URL for the opposite reason — see `postgres::connect`.
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
#[must_use]
pub fn client(config: &Config) -> clickhouse::Client {
    clickhouse::Client::default()
        .with_url(&config.url)
        .with_database(&config.database)
        .with_user(&config.user)
        .with_password(&config.password)
}
