use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::chain::BlockHeader;
use crate::cursor::Cursor;
use crate::fixtures::Trace;
use crate::reorg::{Error, ReorgDetector, Verdict};

/// Test-only detector whose verdicts a case writes, so the loop is what is under test.
#[derive(Clone)]
pub(crate) struct ScriptedReorgDetector {
    state: Arc<Mutex<State>>,
    trace: Trace,
}

struct State {
    /// Consumed in order by successive `inspect` calls; `Continuous` once spent.
    verdicts: Vec<Verdict>,
    bootstrap: Verdict,

    /// `None` by default, so a case takes the inspected path unless it opts out.
    settled: Option<u64>,
    committed: Vec<BlockHeader>,
    inspected: Vec<BlockHeader>,
    rewound_to: Vec<Option<u64>>,
    bootstrapped_with: Vec<Option<Cursor>>,
    given_safe_heads: Vec<Option<u64>>,
}

impl ScriptedReorgDetector {
    pub(crate) fn new(trace: Trace) -> Self {
        Self {
            trace,
            state: Arc::new(Mutex::new(State {
                verdicts: Vec::new(),
                bootstrap: Verdict::Continuous,
                settled: None,
                committed: Vec::new(),
                inspected: Vec::new(),
                rewound_to: Vec::new(),
                bootstrapped_with: Vec::new(),
                given_safe_heads: Vec::new(),
            })),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|held| held.into_inner())
    }

    pub(crate) fn queue(&self, verdicts: impl IntoIterator<Item = Verdict>) {
        self.state().verdicts.extend(verdicts);
    }

    pub(crate) fn on_bootstrap(&self, verdict: Verdict) {
        self.state().bootstrap = verdict;
    }

    pub(crate) fn settled_through(&self, block: u64) {
        self.state().settled = Some(block);
    }

    pub(crate) fn committed(&self) -> Vec<BlockHeader> {
        self.state().committed.clone()
    }

    pub(crate) fn inspected(&self) -> Vec<BlockHeader> {
        self.state().inspected.clone()
    }

    pub(crate) fn rewound_to(&self) -> Vec<Option<u64>> {
        self.state().rewound_to.clone()
    }

    pub(crate) fn bootstrapped_with(&self) -> Vec<Option<Cursor>> {
        self.state().bootstrapped_with.clone()
    }

    pub(crate) fn given_safe_heads(&self) -> Vec<Option<u64>> {
        self.state().given_safe_heads.clone()
    }
}

#[async_trait]
impl ReorgDetector for ScriptedReorgDetector {
    async fn bootstrap(&self, cursor: Option<&Cursor>) -> Result<Verdict, Error> {
        self.trace.note("detector.bootstrap".to_owned());
        let mut state = self.state();
        state.bootstrapped_with.push(cursor.copied());

        Ok(state.bootstrap.clone())
    }

    // Untraced: asked every iteration, it would drown the order the trace shows.
    fn safe_head(&self, _observed_head: u64) -> Option<u64> {
        self.state().settled
    }

    async fn inspect(
        &self,
        header: &BlockHeader,
        safe_head: Option<u64>,
    ) -> Result<Verdict, Error> {
        self.trace
            .note(format!("detector.inspect({})", header.number));
        let mut state = self.state();
        state.inspected.push(*header);
        state.given_safe_heads.push(safe_head);

        if state.verdicts.is_empty() {
            return Ok(Verdict::Continuous);
        }

        Ok(state.verdicts.remove(0))
    }

    async fn commit(&self, header: &BlockHeader, safe_head: Option<u64>) -> Result<(), Error> {
        self.trace
            .note(format!("detector.commit({})", header.number));
        let mut state = self.state();
        state.committed.push(*header);
        state.given_safe_heads.push(safe_head);

        Ok(())
    }

    async fn rewind_to(&self, last_valid_block: Option<u64>) -> Result<(), Error> {
        self.trace.note(format!(
            "detector.rewind_to({})",
            last_valid_block.map_or_else(|| "none".to_owned(), |block| block.to_string())
        ));
        self.state().rewound_to.push(last_valid_block);

        Ok(())
    }
}
