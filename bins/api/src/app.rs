//! Everything a request can reach, in one struct.
//!
//! Three separable steps: an `App` holding the live resources, an `AppState`
//! newtype the router carries, and a `handler` composing state, routes and
//! middleware in that order.
//!
//! Handed its dependencies rather than making them: the [`ShutdownFlag`] has
//! two holders, [`Uptime`] has to be read at the top of `main`, and each client
//! has a second consumer.

use std::sync::Arc;
use std::time::Duration;

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

pub(crate) struct App {
    pub(crate) uptime: Uptime,
    pub(crate) shutdown: ShutdownFlag,

    pub(crate) clickhouse: Client,
    pub(crate) postgres: Pool,

    /// Four ports, four trait objects, so a case can put a double in front of
    /// the handler without a database.
    pub(crate) positions: Box<dyn PositionStore>,
    pub(crate) tokens: Box<dyn TokenMetadataStore>,
    pub(crate) prices: Box<dyn ReservePriceStore>,
    pub(crate) sync: Box<dyn SyncStatusStore>,

    pub(crate) signer: Signer,
    pub(crate) staleness: Staleness,

    /// Read by [`handler`] when it wraps the routes, and nowhere else.
    pub(crate) request_timeout: Duration,

    /// Read once here: it decides where the router mounts, and nothing below
    /// the router asks about it.
    pub(crate) prefix: String,

    pub(crate) docs_path: String,
    pub(crate) docs_assets: String,
}

/// What the router carries, and what every handler extracts.
///
/// The `Arc` is here rather than on the fields, because an `App` carrying
/// `Box<dyn Trait>` cannot be `Clone`. A newtype because `State<_>` needs a
/// type this crate owns.
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
/// The only place the surface is decided: which paths exist, what they hang
/// under and which dependencies a probe answers for are one decision, so they
/// sit beside the resources above rather than in [`crate::router`].
pub(crate) fn handler(app: App) -> Router {
    let (mount, docs) = (router::mount(&app.prefix), router::docs(&app.docs_path));
    let assets = app.docs_assets.clone();
    let request_timeout = app.request_timeout;
    let state = AppState(Arc::new(app));

    // Split before the probes and the document are merged in, so neither
    // appears in what is published.
    let (versioned, api) = OpenApiRouter::with_openapi(openapi::ApiDoc::openapi())
        .nest(&mount, positions::routes().with_state(state.clone()))
        .split_for_parts();

    // The probes and the document take no prefix and no version: all three
    // compose healthchecks ask for `/health/ready`.
    let served = Router::new()
        .merge(probes::routes(state))
        .merge(versioned)
        .merge(docs::routes(&docs, &assets, api));

    middleware::apply(router::refuse_unmatched(served), request_timeout)
}
