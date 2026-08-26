//! Failing readiness before the socket closes.
//!
//! `axum::serve(...).with_graceful_shutdown(f)` stops accepting the moment `f`
//! resolves, which on its own races the endpoints controller: the pod can still
//! be receiving traffic it is no longer listening for. So `f` is
//! [`Drain::on_signal`], which fails readiness *first*, holds for the removal to
//! propagate, and only then resolves — leaving axum to finish what is in flight.
//!
//! A shared flag rather than a service, following
//! [linkerd2-proxy's `Readiness`](https://github.com/linkerd/linkerd2-proxy/blob/main/linkerd/app/admin/src/server/readiness.rs),
//! which is a `Weak<()>` and thirty lines. This one runs in the other
//! direction — ready until told otherwise — so it is an `AtomicBool` rather
//! than a latch, but the shape of the idea is theirs: readiness is a value the
//! handler reads, not a collaborator it calls.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Shared with the readiness handler, and cloned rather than borrowed so the
/// handler and the signal task can both hold one.
#[derive(Clone, Debug, Default)]
pub struct Drain(Arc<AtomicBool>);

impl Drain {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Read once per readiness request.
    ///
    /// `Relaxed`, because nothing is published alongside it: the flag is the
    /// whole message, and a probe that observes the flip one request late is
    /// indistinguishable from one that arrived a millisecond earlier.
    #[must_use]
    pub fn is_draining(&self) -> bool {
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

    /// The future to hand `with_graceful_shutdown`.
    ///
    /// SIGTERM is what an orchestrator sends; SIGINT is what a terminal sends.
    /// Both drain, because a `docker compose down` that skipped the drain would
    /// be a different shutdown path from the deployed one, and the deployed one
    /// is the one that has to work.
    pub async fn on_signal(&self, grace: Duration) {
        let signal = terminated().await;

        // Named, and logged before the hold rather than after it: without this
        // the stream goes quiet for the whole grace window with nothing to say
        // the pod is on its way out. Which signal it was distinguishes an
        // orchestrator from a human at a terminal.
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
        // Nothing can be done about a kernel that will not register the
        // handler, and refusing to serve over it would be worse than draining
        // ungracefully on the day it happens.
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
        let drain = Drain::new();
        assert!(!drain.is_draining());

        drain.begin();
        assert!(drain.is_draining());
    }

    #[test]
    fn a_clone_sees_the_same_flag() {
        // The handler holds one and the signal task holds another; a copy that
        // drained privately would fail readiness for nobody.
        let drain = Drain::new();
        let handler = drain.clone();

        drain.begin();

        assert!(handler.is_draining());
    }

    #[tokio::test(start_paused = true)]
    async fn fails_readiness_before_it_waits_rather_than_after() {
        // The ordering is the whole point of the type: hold first and the pod
        // is still advertised as ready for the length of the grace window.
        let drain = Drain::new();
        let held = tokio::spawn({
            let drain = drain.clone();
            async move { drain.begin_and_hold(Duration::from_secs(10)).await }
        });

        tokio::task::yield_now().await;

        assert!(
            drain.is_draining(),
            "readiness still passing during the hold"
        );
        assert!(!held.is_finished(), "returned without holding");

        tokio::time::advance(Duration::from_secs(10)).await;
        held.await.unwrap();
    }
}
