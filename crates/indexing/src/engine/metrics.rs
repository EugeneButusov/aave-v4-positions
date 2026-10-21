//! What the loop reports about itself.
//!
//! Twelve instruments. The four `rpc.client.*` the transport records are not
//! here, because the transport is not here.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use opentelemetry::metrics::{Counter, Histogram, Meter};
use opentelemetry::{KeyValue, Value};

use crate::engine::Iteration;
use crate::engine::dispatch::Operation;
use crate::engine::status::{IndexerSnapshot, IndexerState, IndexerStatus};

/// Built once, with the meter provider already installed.
///
/// Takes a [`Meter`] rather than naming a scope, so this crate does not link
/// the telemetry crate — and so a case can hand it one over an in-memory
/// exporter.
pub struct Instruments {
    chain: KeyValue,
    iterations: Counter<u64>,
    iteration_duration: Histogram<f64>,
    processor_duration: Histogram<f64>,
    reorg_depth: Histogram<u64>,
    blocks_indexed: Counter<u64>,

    /// Mirrors the loop's own dispatch width, which only ever halves. Held here
    /// rather than read from the loop, because the loop does not expose it and
    /// a getter added purely for a metric is the kind of API a metric should not
    /// be allowed to grow.
    range_size: Arc<Mutex<u64>>,
}

/// Which phase reported a reorg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    Bootstrap,
    Inspect,
}

impl Phase {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Bootstrap => "bootstrap",
            Self::Inspect => "inspect",
        }
    }
}

impl Instruments {
    /// Registers every instrument, including the batch callback the seven gauges
    /// share.
    ///
    /// **The gauges are observable and read [`IndexerStatus`] at collection
    /// time.** That is the load-bearing choice here. Pushing them from the
    /// loop's state transitions looks equivalent and is not: a stalled loop
    /// makes no transitions, so a pushed gauge would freeze at its last value —
    /// reporting healthy numbers precisely when someone is looking because it
    /// stopped. Reading on collection means a wedged indexer's lag keeps
    /// climbing on the graph.
    ///
    /// **One callback for all seven**, so `snapshot` is read once per collection
    /// and cursor, head and lag cannot disagree with each other inside a single
    /// scrape.
    #[must_use]
    pub fn new(
        meter: &Meter,
        chain_id: u32,
        max_range_size: u64,
        status: Arc<IndexerStatus>,
    ) -> Self {
        let chain = KeyValue::new("chain.id", Value::I64(i64::from(chain_id)));
        let range_size = Arc::new(Mutex::new(max_range_size));

        let instruments = Self {
            chain: chain.clone(),
            iterations: meter
                .u64_counter("indexer.iterations")
                .with_description("Loop iterations by outcome. The retry rate, long before the stall alarm.")
                .build(),
            iteration_duration: meter
                .f64_histogram("indexer.iteration.duration")
                .with_unit("s")
                .with_description("How long one pass took, so a slow iteration is distinguishable from a frequent one.")
                .build(),
            processor_duration: meter
                .f64_histogram("indexer.processor.duration")
                .with_unit("s")
                .with_description("Per processor. Dispatch is sequential, so one slow processor is the whole loop.")
                .build(),
            reorg_depth: meter
                .u64_histogram("indexer.reorg.depth")
                .with_unit("{block}")
                .with_description("How deep each reorg reached; its count is how often. Watch it against the finality depth.")
                .build(),
            blocks_indexed: meter
                .u64_counter("indexer.blocks.indexed")
                .with_unit("{block}")
                .with_description("Throughput. Its rate over the lag gauge is the catch-up estimate.")
                .build(),
            range_size: Arc::clone(&range_size),
        };

        observe_status(meter, chain, status, range_size);

        instruments
    }

    fn attributes(&self, outcome: &'static str) -> [KeyValue; 2] {
        [self.chain.clone(), KeyValue::new("outcome", outcome)]
    }

    pub(crate) fn iteration(&self, result: &Iteration, elapsed: Duration) {
        let attributes = self.attributes(result.outcome());

        self.iterations.add(1, &attributes);
        self.iteration_duration
            .record(elapsed.as_secs_f64(), &attributes);
    }

    pub(crate) fn processor(
        &self,
        name: &str,
        operation: Operation,
        outcome: &'static str,
        elapsed: Duration,
    ) {
        self.processor_duration.record(
            elapsed.as_secs_f64(),
            &[
                self.chain.clone(),
                KeyValue::new("processor", name.to_owned()),
                KeyValue::new("operation", operation.as_str()),
                KeyValue::new("outcome", outcome),
            ],
        );
    }

    pub(crate) fn reorg(&self, depth: u64, phase: Phase) {
        self.reorg_depth.record(
            depth,
            &[self.chain.clone(), KeyValue::new("phase", phase.as_str())],
        );
    }

    pub(crate) fn indexed(&self, blocks: u64) {
        self.blocks_indexed
            .add(blocks, std::slice::from_ref(&self.chain));
    }

    pub(crate) fn narrowed_to(&self, size: u64) {
        *self
            .range_size
            .lock()
            .unwrap_or_else(|held| held.into_inner()) = size;
    }
}

/// One snapshot per collection, memoised.
///
/// **There is no batch observable callback in this API** — only one callback per
/// instrument — so the seven gauges cannot literally share a read the way a
/// single batch callback would. Seven independent reads of a moving loop would
/// publish a cursor, a head and a lag from three different moments, and a panel
/// subtracting the first two would disagree with the third.
///
/// So the snapshot is memoised instead, on the only axis available: time. A
/// collection runs its callbacks back to back in microseconds and two scrapes
/// are seconds apart, so any window between those scales gives every gauge in
/// one collection the same snapshot and every collection a fresh one. A
/// millisecond is three orders of magnitude clear of both.
struct PerCollection {
    status: Arc<IndexerStatus>,
    held: Mutex<Option<(Instant, IndexerSnapshot)>>,
}

/// Far longer than a collection cycle, far shorter than a scrape interval.
const ONE_COLLECTION: Duration = Duration::from_millis(1);

impl PerCollection {
    fn snapshot(&self) -> IndexerSnapshot {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some((taken, snapshot)) = held.as_ref()
            && taken.elapsed() < ONE_COLLECTION
        {
            return snapshot.clone();
        }

        let snapshot = self.status.snapshot();
        *held = Some((Instant::now(), snapshot.clone()));

        snapshot
    }
}

fn observe_status(
    meter: &Meter,
    chain: KeyValue,
    status: Arc<IndexerStatus>,
    range_size: Arc<Mutex<u64>>,
) {
    let shared = Arc::new(PerCollection {
        status,
        held: Mutex::new(None),
    });

    gauge_u64(
        meter,
        "indexer.cursor.block",
        "{block}",
        "How far the loop has got, durably.",
        &shared,
        &chain,
        |snapshot| snapshot.last_block,
    );
    gauge_u64(
        meter,
        "indexer.head.block",
        "{block}",
        "The highest head observed, clamped to a high-water mark.",
        &shared,
        &chain,
        |snapshot| snapshot.head,
    );
    gauge_u64(
        meter,
        "indexer.lag.blocks",
        "{block}",
        "Head minus cursor. The alert, and the number that had no way out before.",
        &shared,
        &chain,
        |snapshot| match (snapshot.last_block, snapshot.head) {
            (Some(block), Some(tip)) => Some(tip.saturating_sub(block)),
            _ => None,
        },
    );
    gauge_u64(
        meter,
        "indexer.consecutive_failures",
        "{failure}",
        "Sizes the backoff, so it also says how long until the next attempt.",
        &shared,
        &chain,
        |snapshot| Some(u64::from(snapshot.consecutive_failures)),
    );

    // Not from the status: the loop's own dispatch width, mirrored here.
    {
        let chain = [chain.clone()];
        let range_size = Arc::clone(&range_size);
        meter
            .u64_observable_gauge("indexer.range.size")
            .with_unit("{block}")
            .with_description(
                "Dispatch width, which only ever halves. A falling line is a provider degrading us.",
            )
            .with_callback(move |observer| {
                observer.observe(
                    *range_size.lock().unwrap_or_else(|held| held.into_inner()),
                    &chain,
                );
            })
            .build();
    }

    {
        let (shared, chain) = (Arc::clone(&shared), chain.clone());
        meter
            .f64_observable_gauge("indexer.progress.age")
            .with_unit("s")
            .with_description(
                "Seconds since anything advanced. Graphable before it fails readiness.",
            )
            .with_callback(move |observer| {
                observer.observe(
                    shared.snapshot().progress_age.as_secs_f64(),
                    std::slice::from_ref(&chain),
                );
            })
            .build();
    }

    {
        let (shared, chain) = (Arc::clone(&shared), chain.clone());
        meter
            .u64_observable_gauge("indexer.state")
            .with_description(
                "One series per state, 1 or 0. Alert on failed without parsing a string.",
            )
            .with_callback(move |observer| {
                let snapshot = shared.snapshot();

                // Every state, so the ones it is not in read 0 rather than
                // being absent.
                for candidate in IndexerState::ALL {
                    observer.observe(
                        u64::from(snapshot.state == candidate),
                        &[chain.clone(), KeyValue::new("state", candidate.as_str())],
                    );
                }
            })
            .build();
    }
}

/// **`None` publishes no point rather than a zero.** `last_block` is `None`
/// before anything is indexed and `head` before the chain is read; reporting 0
/// for either would draw a cliff at every restart and make the lag briefly
/// enormous. Reporting nothing draws a gap, which is what it is.
fn gauge_u64(
    meter: &Meter,
    name: &'static str,
    unit: &'static str,
    description: &'static str,
    shared: &Arc<PerCollection>,
    chain: &KeyValue,
    read: fn(&IndexerSnapshot) -> Option<u64>,
) {
    let (shared, chain) = (Arc::clone(shared), [chain.clone()]);

    meter
        .u64_observable_gauge(name)
        .with_unit(unit)
        .with_description(description)
        .with_callback(move |observer| {
            if let Some(value) = read(&shared.snapshot()) {
                observer.observe(value, &chain);
            }
        })
        .build();
}

#[cfg(test)]
mod tests {
    //! The memo the seven gauges share.
    //!
    //! Asserted here rather than through a collection: during one flush the
    //! loop is not moving, so every gauge would agree whether or not they
    //! shared a read. What this holds is the mechanism.

    use super::{ONE_COLLECTION, PerCollection};
    use crate::engine::status::IndexerStatus;
    use std::sync::{Arc, Mutex};

    fn shared(status: &Arc<IndexerStatus>) -> PerCollection {
        PerCollection {
            status: Arc::clone(status),
            held: Mutex::new(None),
        }
    }

    #[test]
    fn every_gauge_in_one_collection_reads_the_same_snapshot() {
        let status = Arc::new(IndexerStatus::new());
        let gauges = shared(&status);
        status.progressed(100);

        let first = gauges.snapshot();
        // The loop moves between two of the seven callbacks, which is exactly
        // the skew that would publish a cursor and a lag from two moments.
        status.progressed(200);
        let second = gauges.snapshot();

        assert_eq!(first.last_block, Some(100));
        assert_eq!(second.last_block, Some(100), "a gauge saw a later loop");
    }

    #[test]
    fn the_next_collection_reads_a_fresh_one() {
        let status = Arc::new(IndexerStatus::new());
        let gauges = shared(&status);
        status.progressed(100);
        let _ = gauges.snapshot();

        status.progressed(200);
        std::thread::sleep(ONE_COLLECTION * 2);

        assert_eq!(gauges.snapshot().last_block, Some(200));
    }
}
