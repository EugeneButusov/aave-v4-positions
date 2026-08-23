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
