use std::sync::Mutex;
use std::time::Duration;

use tokio::time::Instant;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexerState {
    Starting,
    Running,
    Retrying,
    Failed,
    Stopped,
}

impl IndexerState {
    /// The metric attribute and the probe body both read these, so they are one
    /// spelling rather than two.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Retrying => "retrying",
            Self::Failed => "failed",
            Self::Stopped => "stopped",
        }
    }

    /// Every state, so a gauge reports a 0 for the ones it is not in rather than
    /// leaving them absent.
    pub(crate) const ALL: [Self; 5] = [
        Self::Starting,
        Self::Running,
        Self::Retrying,
        Self::Failed,
        Self::Stopped,
    ];
}

/// One read of everything the outside world can learn about the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexerSnapshot {
    pub state: IndexerState,

    /// Why the loop last failed to progress; cleared on the next success.
    pub reason: Option<String>,

    /// Cursor position, or `None` before anything has been indexed.
    pub last_block: Option<u64>,

    /// Highest head observed, or `None` before the chain has been read.
    pub head: Option<u64>,

    /// Since anything advanced. **Taken here rather than exposing the instant**:
    /// both readers want the elapsed time, and computing it from a timestamp
    /// handed over is how one of them ends up measuring a different clock.
    pub progress_age: Duration,
    pub consecutive_failures: u32,
}

/// Everything the outside world can learn about the loop, and the state machine
/// behind it.
///
/// **Not purely a reporting surface**, and it would be dishonest to name it as
/// one: the loop reads [`is_failed`](Self::is_failed) to know it is finished and
/// [`consecutive_failures`](Self::consecutive_failures) to size its backoff.
/// Separating the facts from the facts worth reporting would mean keeping the
/// same values twice.
///
/// What the split buys is a readiness check that depends on this rather than on
/// the whole engine, a state machine that can be driven and asserted without
/// standing up a chain, and a loop left holding only the work.
pub struct IndexerStatus {
    state: Mutex<State>,
}

struct State {
    state: IndexerState,
    reason: Option<String>,
    last_block: Option<u64>,
    head: Option<u64>,
    last_progress_at: Instant,
    failures: u32,
}

impl Default for IndexerStatus {
    fn default() -> Self {
        Self {
            state: Mutex::new(State {
                state: IndexerState::Starting,
                reason: None,
                last_block: None,
                head: None,
                // From construction, so the stall clock starts at boot rather
                // than at the first iteration.
                last_progress_at: Instant::now(),
                failures: 0,
            }),
        }
    }
}

impl IndexerStatus {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Poisoning is ignored rather than refused: a panic while this was held has
    /// already failed the iteration, and a readiness probe that then refused to
    /// answer would turn one bad iteration into a pod nothing can diagnose.
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|held| held.into_inner())
    }

    #[must_use]
    pub fn snapshot(&self) -> IndexerSnapshot {
        let state = self.state();

        IndexerSnapshot {
            state: state.state,
            reason: state.reason.clone(),
            last_block: state.last_block,
            head: state.head,
            progress_age: state.last_progress_at.elapsed(),
            consecutive_failures: state.failures,
        }
    }

    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.state().state == IndexerState::Failed
    }

    #[must_use]
    pub fn failure_reason(&self) -> Option<String> {
        self.state().reason.clone()
    }

    #[must_use]
    pub fn consecutive_failures(&self) -> u32 {
        self.state().failures
    }

    /// Records an observed head and answers with the value the loop should act
    /// on.
    ///
    /// **Clamped to the high-water mark.** Providers are not synchronised, so
    /// failing over to a lagging node makes the head appear to move backwards —
    /// taken at face value that is indistinguishable from a reorg, and it can
    /// invert a range into `from > to`.
    ///
    /// Deliberately not folded into [`progressed`](Self::progressed). The head
    /// is read on every iteration, including ones that go on to idle, retry or
    /// fail, and the loop needs the clamped value back before it knows which of
    /// those this will be. Updating it only on progress would freeze the
    /// reported head exactly when the loop stopped advancing, so a stuck
    /// indexer would understate its own lag.
    pub fn observe_head(&self, observed: u64) -> u64 {
        let mut state = self.state();

        if let Some(previous) = state.head
            && observed < previous
        {
            // A head going backwards *is* a failover: two providers disagreeing
            // about the tip is the only thing that produces one.
            warn!(
                previous_head = previous,
                observed_head = observed,
                "head regressed; clamping to the high mark"
            );
        }

        let clamped = state
            .head
            .map_or(observed, |previous| previous.max(observed));
        state.head = Some(clamped);

        clamped
    }

    pub fn progressed(&self, cursor_at: u64) {
        let mut state = self.state();
        state.last_block = Some(cursor_at);
        advanced(&mut state);
    }

    /// Caught up with nothing to do. **Counts as progress**: being current is
    /// the healthy steady state, and a quiet chain must not trip the stall
    /// alarm.
    pub fn idled(&self) {
        advanced(&mut self.state());
    }

    pub fn retried(&self, reason: String) {
        let mut state = self.state();
        state.state = IndexerState::Retrying;
        state.reason = Some(reason);
        state.failures += 1;
    }

    /// Terminal. Nothing here clears it; a process restart does.
    pub fn failed(&self, reason: String) {
        let mut state = self.state();
        state.state = IndexerState::Failed;
        state.reason = Some(reason);
    }

    pub fn stopped(&self) {
        self.state().state = IndexerState::Stopped;
    }
}

fn advanced(state: &mut State) {
    state.state = IndexerState::Running;
    state.reason = None;
    state.failures = 0;
    state.last_progress_at = Instant::now();
}
