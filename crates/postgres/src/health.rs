use crate::error::Error;
use crate::pool::{Pool, connection};

/// Reports Postgres able to serve a query.
///
/// Deliberately through the pool rather than around it: what a caller needs to
/// know is whether the next *query* can be served, and a pool with no connection
/// to give is as unserviceable as a server that is down.
///
/// Schema integrity belongs to the migration gate, not to a recurring probe.
///
/// # Errors
///
/// [`Error::ConnectFailed`] if no connection can be had, [`Error::PingFailed`]
/// if the server refuses the query.
pub async fn ping(pool: &Pool) -> Result<(), Error> {
    connection(pool)
        .await?
        .simple_query("SELECT 1")
        .await
        .map(drop)
        .map_err(|source| Error::PingFailed { source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{Limits, build_pool};

    fn pool(url: &str) -> Pool {
        build_pool(url, Limits::default()).expect("a parseable URL")
    }

    fn served() -> Pool {
        pool(
            &std::env::var("POSTGRES_URL")
                .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned()),
        )
    }

    #[tokio::test]
    async fn answers_when_the_server_can_serve_a_query() {
        ping(&served()).await.expect("reachable");

        let refused = ping(&pool("postgres://postgres@127.0.0.1:1/nothing"))
            .await
            .expect_err("port 1 is reserved");

        assert_eq!(refused.to_string(), "could not connect to Postgres");
    }
}
