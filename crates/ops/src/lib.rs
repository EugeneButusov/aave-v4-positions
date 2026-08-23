//! What a deployment talks to, in every long-lived process here.
//!
//! Two things, and they are the same thing seen from two sides: `/health/live`
//! and `/health/ready` are how an orchestrator asks whether to route traffic,
//! and [`Drain`] is how the process answers "no" before it stops listening.
//!
//! **No registry, and no `HealthIndicator` trait.** The TypeScript this ports
//! from resolves its indicators through a DI token, which is machinery Rust has
//! nowhere to put — and nothing in the ecosystem replaces it: crates.io's one
//! widely-used health crate is `tonic-health`, and it exists because gRPC
//! standardises a health *protocol*. Services hand-roll the handler. So the
//! report survives the port, because `checks[]` is a wire contract callers
//! read, and the resolution does not: a caller passes one closure listing its
//! dependencies, and [`check`] turns each answer into a row.

mod drain;
mod health;
mod probe;

pub use drain::Drain;
pub use health::{Alive, CheckResult, CheckStatus, Liveness, Readiness, Report, check};
pub use probe::{Uptime, probe_router};
