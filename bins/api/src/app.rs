//! Everything a request can reach, in one struct.
//!
//! Three separable steps: an `App` holding the live resources, an `AppState`
//! newtype the router carries, and a `handler` composing state, routes and
//! middleware in that order.
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
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;

use crate::config::Staleness;
use crate::positions::Signer;
use crate::{docs, middleware, openapi, positions, probes, router};

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
    pub(crate) positions: Box<dyn PositionStore>,
    pub(crate) tokens: Box<dyn TokenMetadataStore>,
    pub(crate) prices: Box<dyn ReservePriceStore>,
    pub(crate) sync: Box<dyn SyncStatusStore>,

    /// The key page cursors are signed with.
    pub(crate) signer: Signer,
    pub(crate) staleness: Staleness,

    /// Read once here rather than carried into every handler: it decides where
    /// the router mounts, and nothing below the router asks about it.
    pub(crate) prefix: String,

    /// Where the document is published, and where the viewer's files are.
    /// Read once here for the same reason.
    pub(crate) docs_path: String,
    pub(crate) docs_assets: String,
}

/// What the router carries, and what every handler extracts.
///
/// The `Arc` is here rather than on the fields: a request clones this, and an
/// `App` carrying `Box<dyn Trait>` cannot be `Clone`.
///
/// A newtype rather than a bare `Arc<App>`, because `State<_>` needs a type
/// this crate owns.
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
    let (mount, docs) = (router::mount(&app.prefix), router::docs(&app.docs_path));
    let assets = app.docs_assets.clone();
    let state = AppState(Arc::new(app));

    // **Split before the probes and the document are merged in**, so neither
    // appears in what is published. The contract describes the versioned API:
    // the probes by the decision `openapi` records, and the two routes that
    // serve the document because a document describing its own address is
    // circular — which is also why the TypeScript's path list has three entries
    // and not five.
    let (versioned, api) = OpenApiRouter::with_openapi(openapi::ApiDoc::openapi())
        .nest(&mount, positions::routes().with_state(state.clone()))
        .split_for_parts();

    // The probes take no prefix and no version: a readiness check is not part
    // of the API's versioned surface, and all three compose healthchecks ask
    // for `/health/ready`. The document takes no prefix either, and for the
    // reason `router::docs` gives.
    let served = Router::new()
        .merge(probes::routes(state))
        .merge(versioned)
        .merge(docs::routes(&docs, &assets, api));

    middleware::apply(router::refuse_unmatched(served))
}
