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

use axum::Router;
use clickhouse_client::clickhouse::Client;
use ops::{ShutdownFlag, Uptime};
use postgres::Pool;

use crate::{middleware, router};

/// The live resources, built once at boot and read for the process's life.
#[derive(Clone)]
pub(crate) struct App {
    pub(crate) uptime: Uptime,
    pub(crate) shutdown: ShutdownFlag,
    pub(crate) clickhouse: Client,
    pub(crate) postgres: Pool,
}

/// What the router carries, and what a handler will extract once one wants it.
///
/// **No `Arc`, while nothing needs one.** Cloning this clones the resources,
/// and one of them is not free: `clickhouse::Client` shares its transport but
/// deep-copies its url, database, auth, roles, settings and headers. The only
/// path that clones per request is the readiness probe, which an orchestrator
/// runs every few seconds — so the cost is a rounding error and the indirection
/// would be speculative.
///
/// It stops being a rounding error when a route serves real traffic and clones
/// this per request. Wrapping the field in an `Arc` at that point is a one-line
/// change here and invisible everywhere else, which is the reason to wait
/// rather than the reason to hurry.
///
/// **crates.io does hold an `Arc` here, and the difference is not taste.** Its
/// `App` cannot be cloned at all — it carries a `HashMap<String, Box<dyn
/// OidcKeyStore>>` among eleven fields, and `Box<dyn Trait>` has no `Clone` — so
/// the indirection is the only way to share it. Ours is four fields that all
/// clone, and the expensive one is expensive by a constant rather than by
/// design. Same shape, different arithmetic.
///
/// A newtype rather than a bare `App` because it is where crates.io hangs
/// `FromRequestParts` and `FromRef`. Those derives wait for a handler that takes
/// state as an argument; the probes reach it through a closure.
#[derive(Clone)]
pub(crate) struct AppState(pub(crate) App);

impl std::ops::Deref for AppState {
    type Target = App;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// State, then routes, then everything wrapped around them.
pub(crate) fn handler(app: App) -> Router {
    let state = AppState(app);

    middleware::apply(router::build(state))
}
