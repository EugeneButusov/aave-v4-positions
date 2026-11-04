use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::fixtures::Trace;
use crate::processors::{BlockProcessor, Failure, Outcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Call {
    pub(crate) operation: &'static str,
    pub(crate) from: u64,
    pub(crate) to: u64,

    /// Read inside the call, which is the only moment it means anything.
    pub(crate) cancelled: bool,
}

/// Test-only processor that records each call and answers from a queue.
#[derive(Clone)]
pub(crate) struct ScriptedBlockProcessor {
    name: String,
    trace: Trace,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    calls: Vec<Call>,

    /// Consumed in order; `Ok` once spent.
    outcomes: Vec<Outcome>,

    /// Checked before the queue and never spent: a defect does not take turns.
    refuse: Option<String>,
}

impl ScriptedBlockProcessor {
    pub(crate) fn new(name: &str, trace: Trace) -> Self {
        Self {
            name: name.to_owned(),
            trace,
            state: Arc::default(),
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
        self.trace
            .note(format!("{}.{operation}({from},{to})", self.name));

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
impl BlockProcessor for ScriptedBlockProcessor {
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
