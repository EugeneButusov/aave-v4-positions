use std::sync::Mutex;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::doubles::detector::{Trace, note};
use crate::processors::{BlockProcessor, Failure, Outcome};

/// What a processor was asked, and whether it had been told to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Call {
    pub(crate) operation: &'static str,
    pub(crate) from: u64,
    pub(crate) to: u64,

    /// Read inside the call, which is the only moment it means anything.
    pub(crate) cancelled: bool,
}

pub(crate) struct RecordingProcessor {
    name: String,
    trace: Trace,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    calls: Vec<Call>,

    /// Consumed in order; `Ok` once spent.
    outcomes: Vec<Outcome>,

    /// Refuses on **every** call until cleared, and is checked before the
    /// queue: a processor with a defect does not take turns.
    refuse: Option<String>,
}

impl RecordingProcessor {
    pub(crate) fn new(name: &str, trace: Trace) -> Self {
        Self {
            name: name.to_owned(),
            trace,
            state: Mutex::new(State::default()),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|held| held.into_inner())
    }

    pub(crate) fn queue(&self, outcomes: impl IntoIterator<Item = Outcome>) {
        self.state().outcomes.extend(outcomes);
    }

    pub(crate) fn refuse_with(&self, reason: &str) {
        self.state().refuse = Some(reason.to_owned());
    }

    pub(crate) fn calls(&self) -> Vec<Call> {
        self.state().calls.clone()
    }

    /// Just the block ranges, which is what most cases assert on.
    pub(crate) fn ranges(&self) -> Vec<(u64, u64)> {
        self.calls()
            .iter()
            .filter(|call| call.operation == "block-range")
            .map(|call| (call.from, call.to))
            .collect()
    }

    fn answer(
        &self,
        operation: &'static str,
        from: u64,
        to: u64,
        cancel: &CancellationToken,
    ) -> Result<Outcome, Failure> {
        note(
            &self.trace,
            format!("{}.{operation}({from},{to})", self.name),
        );

        let mut state = self.state();
        state.calls.push(Call {
            operation,
            from,
            to,
            cancelled: cancel.is_cancelled(),
        });

        if let Some(reason) = &state.refuse {
            return Err(reason.clone().into());
        }

        if state.outcomes.is_empty() {
            return Ok(Outcome::Ok);
        }

        Ok(state.outcomes.remove(0))
    }
}

#[async_trait]
impl BlockProcessor for RecordingProcessor {
    fn name(&self) -> &str {
        &self.name
    }

    async fn on_block_range(
        &self,
        from: u64,
        to: u64,
        cancel: &CancellationToken,
    ) -> Result<Outcome, Failure> {
        self.answer("block-range", from, to, cancel)
    }

    async fn on_reorg(
        &self,
        from: u64,
        to: u64,
        cancel: &CancellationToken,
    ) -> Result<Outcome, Failure> {
        self.answer("reorg", from, to, cancel)
    }
}
