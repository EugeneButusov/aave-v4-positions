//! Everything a request can reach, in one struct.
//!
//! The shape is [crates.io's](https://github.com/rust-lang/crates.io/blob/main/src/lib.rs):
//! an `App` holding the live resources, an `AppState` newtype the router
//! carries, and a `handler` composing state, routes and middleware in that
//! order.
//!
//! **It is handed its dependencies rather than making them.** The
//! [`ShutdownFlag`] has two holders — the readiness handler and the future
//! `with_graceful_shutdown` waits on — so one made here could never be flipped
//! by the other. [`Uptime`] is read at the top of `main`, or it would start
//! counting after the config and both clients. The clients are passed in because
//! each has a second consumer: the position store takes the ClickHouse client,
//! the three read stores take the pool.

use std::sync::Arc;

use aave_positions::store::PositionStore;
use axum::Router;
use clickhouse_client::clickhouse::Client;
use indexing::SyncStatusStore;
use ops::{ShutdownFlag, Uptime};
use postgres::Pool;
use prices::ReservePriceStore;
use token_metadata::TokenMetadataStore;

use crate::config::Staleness;
use crate::positions::Signer;
use crate::{middleware, positions, router};

/// The live resources, built once at boot and read for the process's life.
pub(crate) struct App {
    pub(crate) uptime: Uptime,
    pub(crate) shutdown: ShutdownFlag,

    /// Held for the readiness probe, and to build the position store from.
    pub(crate) clickhouse: Client,
    pub(crate) postgres: Pool,

    /// **Four ports, four trait objects**, so a case can put a double in front
    /// of the handler without a database. A boxed future per call is the cost,
    /// which `PositionStore`'s own doc measures and accepts.
    pub(crate) positions: Arc<dyn PositionStore>,
    pub(crate) tokens: Arc<dyn TokenMetadataStore>,
    pub(crate) prices: Arc<dyn ReservePriceStore>,
    pub(crate) sync: Arc<dyn SyncStatusStore>,

    /// The key page cursors are signed with.
    pub(crate) signer: Signer,
    pub(crate) staleness: Staleness,

    /// Read once here rather than carried into every handler: it decides where
    /// the router mounts, and nothing below the router asks about it.
    pub(crate) prefix: String,
}

/// What the router carries, and what every handler extracts.
///
/// **An `Arc`, now that a route serves real traffic and clones this per
/// request.** It held an `App` directly while only the readiness probe cloned
/// it, with a note saying this would be a one-line change when a handler
/// arrived. It is that line, and it is also what makes the four trait objects
/// shareable at all — an `App` carrying `Box<dyn Trait>` cannot be `Clone`.
///
/// A newtype rather than a bare `Arc<App>`: it is where crates.io hangs
/// `FromRequestParts` and `FromRef`, and `State<_>` needs a type this crate
/// owns.
#[derive(Clone)]
pub(crate) struct AppState(pub(crate) Arc<App>);

impl std::ops::Deref for AppState {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// State, then routes, then everything wrapped around them.
///
/// **This is where the surface is decided, and the only place.** Which paths
/// exist, what they hang under and which dependencies a probe answers for are
/// one decision — what this process *is* — so they sit beside the resources
/// above rather than in [`crate::router`], which names nothing this service does.
pub(crate) fn handler(app: App) -> Router {
    let mount = router::mount(&app.prefix);
    let (uptime, shutdown) = (app.uptime, app.shutdown.clone());
    let state = AppState(Arc::new(app));

    // A router, then what it answers on. **Neither half is the base**: starting
    // from the probes and nesting the rest into them reads as though an
    // orchestrator's check were the service and the API hung off it.
    //
    // The probes take no prefix and no version — a readiness check is not part
    // of the API's versioned surface, and all three compose healthchecks
    // already ask for `/health/ready`.
    let served = Router::new()
        .merge(probes(uptime, shutdown, state.clone()))
        .nest(&mount, positions::routes().with_state(state));

    middleware::apply(router::refuse_unmatched(served))
}

/// **The dependencies are named here, in a list, and that is the whole
/// registry.** `ops` owns the report and the paths; this owns which databases
/// this process answers for.
fn probes(uptime: Uptime, shutdown: ShutdownFlag, state: AppState) -> Router {
    ops::probe_router(uptime, shutdown, move || {
        let state = state.clone();
        async move {
            // Side by side: a probe that serialises them reports the sum of
            // two timeouts. `join!` keeps the order the wire contract fixes.
            let (clickhouse, postgres) = tokio::join!(
                ops::check("clickhouse", clickhouse_client::ping(&state.clickhouse)),
                ops::check("postgres", ::postgres::ping(&state.postgres)),
            );
            vec![clickhouse, postgres]
        }
    })
}
