//! How long the loop waits: between iterations, and for a drain.

use std::time::Duration;

use crate::engine::Iteration;

/// Constants rather than configuration: retries are unbounded, and the stall
/// signal rather than the delay is what makes a wedged loop visible, so there is
/// nothing here an operator would usefully tune.
const RETRY_BASE: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// How long a shutdown waits for an iteration that ignored its cancellation.
///
/// Exists so a processor that does not honour the token cannot make the drain
/// outlive the container's termination grace period.
pub const SHUTDOWN_DRAIN: Duration = Duration::from_secs(10);

/// How long to wait before the next iteration.
pub(crate) fn delay_after(result: &Iteration, poll_interval: Duration, failures: u32) -> Duration {
    match result {
        Iteration::Progressed { .. } => Duration::ZERO,
        Iteration::Idle => poll_interval,
        // `Failed` is unreachable from `run`, which returns before sleeping on it.
        Iteration::Retry { .. } | Iteration::Failed { .. } => backoff(failures),
    }
}

/// How long to wait after the `failures`th consecutive failure.
///
/// 500ms doubling to a 30s ceiling, with no jitter: a defect kept on purpose, see
/// `docs/rust-migration.md`.
pub(crate) fn backoff(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(16);

    RETRY_MAX.min(RETRY_BASE * 2_u32.pow(exponent))
}
