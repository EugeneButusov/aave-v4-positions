//! Serves the read API.
//!
//! The first long-lived process in this workspace, which is why `ops` exists and
//! why the shutdown path is written out rather than left to the runtime:
//! `bins/migrate` runs to completion and has nothing to drain.
//!
//! Boot order is config, logging, dependencies, state, listener — one parsed
//! configuration flowing downward, and nothing reading the environment behind
//! it. The TypeScript could not do this: its telemetry SDK is preloaded and
//! reads `process.env` before Nest exists, which is what its `env.ts` spends a
//! paragraph explaining. That paragraph has nothing to describe here.
//!
//! What this file does **not** do is name a route or a layer. Those are
//! [`router`] and [`middleware`], composed by [`app::handler`].

mod app;
mod config;
mod errors;
mod logging;
mod middleware;
mod router;
#[cfg(test)]
mod test_support;

use std::error::Error;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::Arc;

use app::App;
use config::Config;
use ops::{Drain, Uptime};

#[tokio::main]
async fn main() -> ExitCode {
    // Before anything else, so `uptimeSeconds` counts from the process rather
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
    // A local-development convenience and nothing more: every deployed
    // environment injects variables directly, and the image carries no file for
    // this to find. Ignoring the error is the normal case, not a swallowed
    // failure. It does not override a variable that is already set, which is
    // also what `@nestjs/config` does with the same file — the repo's own
    // `cp .env.example .env` workflow depends on both halves.
    let _ = dotenvy::dotenv();

    let config = Config::from_env()?;
    logging::init(config.level, config.pretty);

    // Neither of these dials. ClickHouse's client is lazy by construction and
    // the Postgres pool is lazy by choice, so a database that is briefly down
    // means a process that boots and reports itself not-ready rather than one
    // that crash-loops.
    let clickhouse = clickhouse_client::build_client(config.clickhouse);
    let postgres = postgres::build_pool(&config.postgres_url)?;

    let drain = Drain::new();
    let handler = app::handler(Arc::new(App {
        uptime,
        drain: drain.clone(),
        clickhouse,
        postgres,
    }));

    let address = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "api listening");

    axum::serve(listener, handler)
        .with_graceful_shutdown(async move { drain.on_signal(config.grace).await })
        .await?;

    // Reached only once the accept loop has stopped and the last in-flight
    // request has been answered.
    tracing::info!("shutdown complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The one case that goes through a socket rather than around it.
    //!
    //! Everything in `router` and `middleware` drives the router directly, which
    //! leaves `axum::serve`, the listener and the shutdown future untested — and
    //! those are this file's whole contribution. One request over TCP, then the
    //! drain, covers it without an HTTP client dependency: the response is read
    //! as bytes because the only thing asserted is that the process answered.

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::test_support::{clickhouse, postgres};

    #[tokio::test]
    async fn serves_over_a_socket_and_stops_when_drained() {
        let drain = Drain::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        // Stands in for the signal, so the drain happens after the request
        // rather than racing it.
        let (terminate, terminated) = tokio::sync::oneshot::channel::<()>();

        let served = tokio::spawn({
            let handler = app::handler(Arc::new(App {
                uptime: Uptime::now(),
                drain: drain.clone(),
                clickhouse: clickhouse(),
                postgres: postgres(),
            }));
            let drain = drain.clone();
            async move {
                axum::serve(listener, handler)
                    .with_graceful_shutdown(async move {
                        let _ = terminated.await;
                        drain.begin_and_hold(std::time::Duration::ZERO).await;
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
            response.ends_with(r#"{"status":"ok","uptimeSeconds":0}"#),
            "{response}"
        );

        // Resolving the shutdown future is what ends the accept loop; a server
        // that ignored it would hang here rather than fail.
        terminate.send(()).unwrap();
        served.await.unwrap();
    }
}
