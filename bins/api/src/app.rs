//! Everything a request can reach, in one struct.
//!
//! The shape is [crates.io's](https://github.com/rust-lang/crates.io/blob/main/src/lib.rs): an `App`
//! holding the live resources, an `AppState` newtype the router carries, and a
//! `handler` that composes state, routes and middleware in that order. Three
//! separable steps, and the reason to adopt it before the positions endpoint
//! rather than after is that the endpoint then lands *into* a structure instead
//! of forcing one.
//!
//! **It is handed its dependencies rather than making them**, and each has its
//! own reason. The [`ShutdownFlag`] has two holders — the readiness handler and the
//! future `with_graceful_shutdown` waits on — so one made in here could never be
//! flipped by the other. [`Uptime`] is read at the top of `main`, because
//! started here it would begin counting after the config and both clients. The
//! clients could be built from a [`crate::config::Config`] and are not, because
//! each is about to have a second consumer: the position store takes the
//! ClickHouse client, the read stores take the pool, and a constructor that made
//! them would have to make those too — at which point it is the composition root
//! rather than a description of what is served.

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
use crate::positions::Cursors;
use crate::{middleware, router};

/// The live resources, built once at boot and read for the process's life.
pub(crate) struct App {
    pub(crate) uptime: Uptime,
    pub(crate) shutdown: ShutdownFlag,

    /// Held for the readiness probe, and to build the position store from.
    pub(crate) clickhouse: Client,
    pub(crate) postgres: Pool,

    /// **Four ports, four trait objects.** The composition root reads like the
    /// module graph it replaces, and a case can put a double in front of the
    /// handler without a database. The cost is a boxed future per call, which
    /// `PositionStore`'s own doc measures and accepts.
    pub(crate) positions: Arc<dyn PositionStore>,
    pub(crate) tokens: Arc<dyn TokenMetadataStore>,
    pub(crate) prices: Arc<dyn ReservePriceStore>,
    pub(crate) sync: Arc<dyn SyncStatusStore>,

    pub(crate) cursors: Cursors,
    pub(crate) staleness: Staleness,

    /// Read once here rather than carried into every handler: it decides where
    /// the router mounts, and nothing below the router asks about it.
    pub(crate) prefix: String,
}

/// What the router carries, and what every handler extracts.
///
/// **An `Arc`, now that a route serves real traffic and clones this per
/// request.** This used to hold an `App` directly, on the argument that cloning
/// four fields cost a rounding error while only the readiness probe did it — a
/// `clickhouse::Client` shares its transport but deep-copies its url, database,
/// auth, roles, settings and headers, and an orchestrator asks every few
/// seconds. The note beside it said wrapping the field would be a one-line
/// change when a handler arrived. It is this line, and it is also what makes the
/// four trait objects shareable at all.
///
/// This is now crates.io's shape for the same reason theirs has it: an `App`
/// carrying `Box<dyn Trait>` cannot be `Clone`.
///
/// A newtype rather than a bare `Arc<App>` because it is where crates.io hangs
/// `FromRequestParts` and `FromRef` — and `State<AppState>` needs a type this
/// crate owns.
#[derive(Clone)]
pub(crate) struct AppState(pub(crate) Arc<App>);

impl App {
    /// Where the versioned routes are mounted, with the slashes a deployment may
    /// or may not have written. An empty prefix mounts them at `/v1`, which is a
    /// deployment saying it wants no prefix rather than one asking for `//v1`.
    fn prefix(&self) -> String {
        self.prefix.trim_matches('/').to_owned()
    }
}

impl std::ops::Deref for AppState {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// State, then routes, then everything wrapped around them.
pub(crate) fn handler(app: App) -> Router {
    let prefix = app.prefix();
    let state = AppState(Arc::new(app));

    middleware::apply(router::build(state, &prefix))
}
