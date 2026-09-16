//! Serves the read API.
//!
//! The first long-lived process in this workspace, which is why `ops` exists and
//! why the shutdown path is written out rather than left to the runtime:
//! `bins/migrate` runs to completion and has nothing to shutdown.
//!
//! Boot order is config, telemetry, dependencies, state, listener — one parsed
//! configuration flowing downward, and nothing reading the environment behind
//! it. That includes the `OTEL_*` group: the SDK this ports from had to read it
//! for itself, before the application existed, and nothing here has to.
//!
//! What this file does **not** do is name a route or a layer. [`app::handler`]
//! composes them.

mod app;
mod config;
mod docs;
mod errors;
mod middleware;
mod openapi;
mod positions;
mod probes;
mod router;
#[cfg(test)]
mod test_support;

use std::error::Error;
use std::net::SocketAddr;
use std::process::ExitCode;

use aave_positions::store::ClickHousePositionStore;
use indexing::PostgresSyncStatusStore;
use ops::{ShutdownFlag, Uptime};
use prices::PostgresReservePriceStore;
use token_metadata::PostgresTokenMetadataStore;

use app::App;
use config::Config;
use positions::Signer;

#[tokio::main]
async fn main() -> ExitCode {
    // Before anything else, so `uptime_seconds` counts from the process rather
    // than from whenever the databases finished being dialled.
    let uptime = Uptime::now();

    match run(uptime).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // The chain, not just the head, and on stderr rather than through
            // `tracing`: the most likely failure here is the configuration that
            // would have told us how to log.
            eprintln!("api: {error}");
            let mut cause = error.source();
            while let Some(next) = cause {
                eprintln!("  caused by: {next}");
                cause = next.source();
            }
            ExitCode::FAILURE
        }
    }
}

async fn run(uptime: Uptime) -> Result<(), Box<dyn Error>> {
    let config = Config::from_env()?;
    let telemetry = telemetry::init(config.telemetry)?;

    // Neither of these dials. ClickHouse's client is lazy by construction and
    // the Postgres pool is lazy by choice, so a database that is briefly down
    // means a process that boots and reports itself not-ready rather than one
    // that crash-loops.
    let clickhouse = clickhouse_client::build_client(config.clickhouse);
    let postgres = postgres::build_pool(&config.postgres_url)?;

    let shutdown = ShutdownFlag::new();

    let document = format!("{}/openapi.json", router::docs(&config.docs_path));

    let handler = app::handler(App {
        uptime,
        shutdown: shutdown.clone(),
        positions: Box::new(ClickHousePositionStore::new(clickhouse.clone())),
        tokens: Box::new(PostgresTokenMetadataStore::new(postgres.clone())),
        prices: Box::new(PostgresReservePriceStore::new(postgres.clone())),
        sync: Box::new(PostgresSyncStatusStore::new(postgres.clone())),
        signer: Signer::new(&config.cursor_secret)?,
        staleness: config.staleness,
        prefix: config.prefix,
        docs_path: config.docs_path,
        docs_assets: config.docs_assets,
        clickhouse,
        postgres,
    });

    let address = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, %document, "api listening");

    axum::serve(listener, handler)
        .with_graceful_shutdown(async move { shutdown.on_signal(config.grace).await })
        .await?;

    // Reached only once the accept loop has stopped and the last in-flight
    // request has been answered.
    tracing::info!("shutdown complete");

    // Last, and off a worker: the flush blocks on threads that post their final
    // batch back through this runtime, so holding a worker here is how that
    // deadlocks. The line above goes with it.
    tokio::task::spawn_blocking(move || telemetry.shutdown()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The one case that goes through a socket rather than around it.
    //!
    //! Everything in `router` and `middleware` drives the router directly, which
    //! leaves `axum::serve`, the listener and the shutdown future untested — and
    //! those are this file's whole contribution. One request over TCP, then the
    //! shutdown, covers it without an HTTP client dependency: the response is read
    //! as bytes because the only thing asserted is that the process answered.

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::test_support::{postgres, state};

    #[tokio::test]
    async fn serves_over_a_socket_and_stops_when_drained() {
        let shutdown = ShutdownFlag::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        // Stands in for the signal, so the shutdown happens after the request
        // rather than racing it.
        let (terminate, terminated) = tokio::sync::oneshot::channel::<()>();

        let served = tokio::spawn({
            // The flag this case holds, rather than the one `state` makes:
            // the readiness handler and the future `with_graceful_shutdown`
            // waits on have to be looking at the same one.
            let handler = app::handler(App {
                shutdown: shutdown.clone(),
                ..state(postgres())
            });
            let shutdown = shutdown.clone();
            async move {
                axum::serve(listener, handler)
                    .with_graceful_shutdown(async move {
                        let _ = terminated.await;
                        shutdown.begin_and_hold(std::time::Duration::ZERO).await;
                    })
                    .await
                    .unwrap();
            }
        });

        let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
        socket
            .write_all(b"GET /health/live HTTP/1.1\r\nHost: probe\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();

        let mut response = String::new();
        socket.read_to_string(&mut response).await.unwrap();

        assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
        assert!(
            response.ends_with(r#"{"status":"ok","uptime_seconds":0}"#),
            "{response}"
        );

        // Resolving the shutdown future is what ends the accept loop; a server
        // that ignored it would hang here rather than fail.
        terminate.send(()).unwrap();
        served.await.unwrap();
    }
}
