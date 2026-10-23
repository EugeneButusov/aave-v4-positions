//! What a deployment talks to, in every long-lived process here.
//!
//! `/health/live` and `/health/ready` are how an orchestrator asks whether to
//! route traffic; [`ShutdownFlag`] is how the process answers "no" before it
//! stops listening. No registry and no indicator trait — a caller passes one
//! closure listing its dependencies and [`check`] turns each answer into a row.
//!
//! [`error_message_with_causes`] renders the `error` on a failed check, and the
//! line `bins/api` logs when a request fails.

mod cause;
mod health;
mod probe;
mod shutdown;

pub use cause::error_message_with_causes;
pub use health::{Alive, CheckResult, CheckStatus, Liveness, Readiness, Report, check};
pub use probe::{Uptime, is_probe, probe_router};
pub use shutdown::ShutdownFlag;
