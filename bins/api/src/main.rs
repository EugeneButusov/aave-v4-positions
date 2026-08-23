//! Serves the read API.
//!
//! The first long-lived process in this workspace, which is why `ops` exists
//! and why the shutdown path is written out rather than left to the runtime:
//! `bins/migrate` runs to completion and has nothing to drain.
//!
//! Boot order is config, logging, dependencies, routes, listener — one parsed
//! configuration flowing downward, and nothing reading the environment behind
//! it. The TypeScript could not do this: its telemetry SDK is preloaded and
//! reads `process.env` before Nest exists, which is what its `env.ts` spends a
//! paragraph explaining. That paragraph has nothing to describe here.

mod app;
mod config;
mod logging;

use std::error::Error;
use std::net::SocketAddr;
use std::process::ExitCode;

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
    let app = app::router(uptime, drain.clone(), clickhouse, postgres);

    let address = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address, "api listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { drain.on_signal(config.grace).await })
        .await?;

    // Reached only once the accept loop has stopped and the last in-flight
    // request has been answered.
    tracing::info!("shutdown complete");
    Ok(())
}
