//! Failing readiness before the socket closes.
//!
//! `with_graceful_shutdown(f)` stops accepting the moment `f` resolves, which
//! races the endpoints controller: the pod can still be receiving traffic it is
//! no longer listening for. So `f` is [`ShutdownFlag::on_signal`], which fails
//! readiness first, holds for the removal to propagate, and only then resolves.
//!
//! A shared flag rather than a service, after linkerd2-proxy's `Readiness`:
//! readiness is a value the handler reads, not a collaborator it calls.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Shared with the readiness handler, and cloned rather than borrowed so the
/// handler and the signal task can both hold one.
#[derive(Clone, Debug, Default)]
pub struct ShutdownFlag(Arc<AtomicBool>);

impl ShutdownFlag {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read once per readiness request. `Relaxed`, because the flag is the
    /// whole message and nothing is published alongside it.
    #[must_use]
    pub fn has_begun(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Starts failing readiness. Idempotent, and there is no way back.
    pub fn begin(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Fails readiness, then waits out the window before returning.
    ///
    /// Split from [`Self::on_signal`] so the hold can be tested without raising
    /// a signal at the test process.
    pub async fn begin_and_hold(&self, grace: Duration) {
        self.begin();
        tokio::time::sleep(grace).await;
    }

    /// The future to hand `with_graceful_shutdown`. Both SIGTERM and SIGINT,
    /// so `docker compose down` takes the deployed path rather than its own.
    pub async fn on_signal(&self, grace: Duration) {
        let signal = terminated().await;

        // Logged before the hold, or the stream goes quiet for the whole grace
        // window with nothing to say the pod is on its way out.
        tracing::info!(
            signal,
            grace_seconds = grace.as_secs(),
            "failing readiness before close"
        );

        self.begin_and_hold(grace).await;
    }
}

#[cfg(unix)]
async fn terminated() -> &'static str {
    use tokio::signal::unix::{SignalKind, signal};

    // Both handlers are installed before either is awaited, so a signal
    // arriving between the two is still caught.
    let Ok(mut term) = signal(SignalKind::terminate()) else {
        // Refusing to serve over an unregistrable handler would be worse than
        // draining ungracefully on the day it happens.
        return std::future::pending().await;
    };

    tokio::select! {
        _ = term.recv() => "SIGTERM",
        result = tokio::signal::ctrl_c() => {
            let _ = result;
            "SIGINT"
        }
    }
}

#[cfg(not(unix))]
async fn terminated() -> &'static str {
    let _ = tokio::signal::ctrl_c().await;
    "SIGINT"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_ready_until_told_otherwise() {
        let shutdown = ShutdownFlag::new();
        assert!(!shutdown.has_begun());

        shutdown.begin();
        assert!(shutdown.has_begun());
    }

    #[test]
    fn a_clone_sees_the_same_flag() {
        // A copy that drained privately would fail readiness for nobody.
        let shutdown = ShutdownFlag::new();
        let handler = shutdown.clone();

        shutdown.begin();

        assert!(handler.has_begun());
    }

    #[tokio::test(start_paused = true)]
    async fn fails_readiness_before_it_waits_rather_than_after() {
        // Hold first and the pod stays advertised as ready for the window.
        let shutdown = ShutdownFlag::new();
        let held = tokio::spawn({
            let shutdown = shutdown.clone();
            async move { shutdown.begin_and_hold(Duration::from_secs(10)).await }
        });

        tokio::task::yield_now().await;

        assert!(
            shutdown.has_begun(),
            "readiness still passing during the hold"
        );
        assert!(!held.is_finished(), "returned without holding");

        tokio::time::advance(Duration::from_secs(10)).await;
        held.await.unwrap();
    }
}
