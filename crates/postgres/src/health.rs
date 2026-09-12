use crate::error::Error;
use crate::pool::{Pool, connection};

/// Reports the state store reachable.
///
/// `SELECT 1`, for the same reason `clickhouse_client::ping` uses one, and it
/// deliberately goes through the pool rather than around it: what a caller needs
/// to know is whether the next *query* can be served, and a pool with no
/// connection to give is as unserviceable as a server that is down.
///
/// # Errors
///
/// [`Error::ConnectFailed`] if no connection can be had, [`Error::PingFailed`] if
/// the server refuses the query.
pub async fn ping(pool: &Pool) -> Result<(), Error> {
    connection(pool)
        .await?
        .simple_query("SELECT 1")
        .await
        .map(drop)
        .map_err(|source| Error::PingFailed { source })
}
