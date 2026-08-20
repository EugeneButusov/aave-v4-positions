use crate::error::Error;

/// What [`build_client`] hands back.
///
/// An alias, not a wrapper: refinery implements its traits for this exact type,
/// so anything of ours in between would have to delegate them straight back. It
/// exists so a caller can name the return value — hold one in a struct, write a
/// helper that takes one — without declaring the driver a second time, and
/// without this crate re-exporting a whole driver to expose one name.
pub type Client = tokio_postgres::Client;

/// Opens a connection and starts the task that drives it.
///
/// One URL rather than the four discrete parts ClickHouse takes. Every managed
/// Postgres hands you exactly this string, often carrying `?sslmode=require`, and
/// splitting it into parts means reassembling it — the first thing reassembly
/// gets wrong is percent-encoding a password.
///
/// **`NoTls`, which is a gap and not a decision.** It is enough for the container
/// this is developed and tested against, and it is not enough for a managed
/// server that requires SSL. The TLS connector arrives with the first deployment
/// that needs one; nothing here would have exercised it.
///
/// # Errors
///
/// [`Error::ConnectFailed`] if the URL will not parse or the server will not have us.
pub async fn build_client(url: &str) -> Result<Client, Error> {
    let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .map_err(|source| Error::ConnectFailed { source })?;

    // tokio-postgres hands back the client and the connection separately: the
    // client only queues messages, and this task is what drives the socket. Skip
    // it and every query hangs forever.
    //
    // The error is dropped because there is nowhere to put it yet — the next use
    // of the client fails with the same cause attached, so nothing is lost but
    // the timing. It becomes a `tracing::error!` when `telemetry` lands.
    tokio::spawn(async move { drop(connection.await) });

    Ok(client)
}
