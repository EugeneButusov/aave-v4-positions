use std::sync::Mutex;

use async_trait::async_trait;

use crate::chain::BlockHeader;
use crate::cursor::Cursor;
use crate::reorg::{Error, ReorgDetector, Verdict};

/// A detector whose verdicts a case writes, so the loop's own behaviour is what
/// is under test rather than the detector's.
///
/// The real one has its own suite; this exists so a case can put the loop in
/// front of a fork without constructing one.
pub(crate) struct ScriptedDetector {
    state: Mutex<State>,
    trace: Trace,
}

struct State {
    /// Consumed in order by successive `inspect` calls; `Continuous` once spent.
    verdicts: Vec<Verdict>,
    bootstrap: Verdict,

    /// **Nothing is settled by default**, so a case takes the inspected path
    /// unless it opts out.
    settled: Option<u64>,
    committed: Vec<BlockHeader>,
    inspected: Vec<BlockHeader>,
    rewound_to: Vec<Option<u64>>,
    bootstrapped_with: Vec<Option<Cursor>>,
}

/// The shared order-of-operations log.
///
/// **The only way to prove the reorg check happens *before* dispatch** rather
/// than alongside it: the detector and the processors write into one list, so a
/// case reads the sequence rather than inferring it.
pub(crate) type Trace = std::sync::Arc<Mutex<Vec<String>>>;

pub(crate) fn trace() -> Trace {
    std::sync::Arc::new(Mutex::new(Vec::new()))
}

pub(crate) fn recorded(trace: &Trace) -> Vec<String> {
    trace
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .clone()
}

pub(crate) fn note(trace: &Trace, entry: String) {
    trace
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .push(entry);
}

impl ScriptedDetector {
    pub(crate) fn new(trace: Trace) -> Self {
        Self {
            trace,
            state: Mutex::new(State {
                verdicts: Vec::new(),
                bootstrap: Verdict::Continuous,
                settled: None,
                committed: Vec::new(),
                inspected: Vec::new(),
                rewound_to: Vec::new(),
                bootstrapped_with: Vec::new(),
            }),
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

    /// Everything at or below `block` is settled, so the loop dispatches in
    /// ranges.
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
}

#[async_trait]
impl ReorgDetector for ScriptedDetector {
    async fn bootstrap(&self, cursor: Option<&Cursor>) -> Result<Verdict, Error> {
        note(&self.trace, "detector.bootstrap".to_owned());
        let mut state = self.state();
        state.bootstrapped_with.push(cursor.copied());

        Ok(state.bootstrap.clone())
    }

    /// Deliberately not traced: the loop asks it every iteration, and it would
    /// drown the order the trace exists to show.
    async fn safe_head(&self, _observed_head: u64) -> Result<Option<u64>, Error> {
        Ok(self.state().settled)
    }

    async fn inspect(&self, header: &BlockHeader) -> Result<Verdict, Error> {
        note(&self.trace, format!("detector.inspect({})", header.number));
        let mut state = self.state();
        state.inspected.push(*header);

        if state.verdicts.is_empty() {
            return Ok(Verdict::Continuous);
        }

        Ok(state.verdicts.remove(0))
    }

    async fn commit(&self, header: &BlockHeader) -> Result<(), Error> {
        note(&self.trace, format!("detector.commit({})", header.number));
        self.state().committed.push(*header);

        Ok(())
    }

    async fn rewind_to(&self, last_valid_block: Option<u64>) -> Result<(), Error> {
        note(
            &self.trace,
            format!(
                "detector.rewind_to({})",
                last_valid_block.map_or_else(|| "none".to_owned(), |block| block.to_string())
            ),
        );
        self.state().rewound_to.push(last_valid_block);

        Ok(())
    }
}
