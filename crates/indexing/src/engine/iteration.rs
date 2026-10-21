/// What one iteration accomplished.
///
/// Drives both the backoff and, through the status it is recorded into, the
/// stall signal on readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use]
pub enum Iteration {
    /// Work was dispatched and the cursor moved. A reorg rewind counts.
    Progressed {
        cursor_at: u64,
    },

    /// Caught up to the head; nothing to do until a new block arrives.
    Idle,
    Retry {
        reason: String,
    },
    Failed {
        reason: String,
    },
}

impl Iteration {
    /// The metric attribute and the span attribute both read this.
    pub(crate) fn outcome(&self) -> &'static str {
        match self {
            Self::Progressed { .. } => "progressed",
            Self::Idle => "idle",
            Self::Retry { .. } => "retry",
            Self::Failed { .. } => "failed",
        }
    }
}
