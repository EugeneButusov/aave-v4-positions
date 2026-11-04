use std::collections::BTreeSet;

use opentelemetry_sdk::metrics::data::AggregatedMetrics;

use crate::engine::harness::{Fixture, HEAD, MAX_RANGE, failed, narrowing, plain, retry};
use crate::fixtures::hash_of;
use crate::reorg::Verdict;
/// **Resets before flushing.** `get_finished_metrics` accumulates every
/// flush, so without this a second read sees the first collection's points
/// as well and `[0]` is whichever is stale.
fn collected(fixture: &Fixture) -> Vec<opentelemetry_sdk::metrics::data::ResourceMetrics> {
    fixture.exporter.reset();
    fixture.provider.force_flush().expect("a collection");

    fixture
        .exporter
        .get_finished_metrics()
        .expect("the exported batches")
}

fn instrument_names(fixture: &Fixture) -> BTreeSet<String> {
    collected(fixture)
        .iter()
        .flat_map(|batch| batch.scope_metrics().collect::<Vec<_>>())
        .flat_map(|scope| scope.metrics().collect::<Vec<_>>())
        .map(|metric| metric.name().to_owned())
        .collect()
}

/// The whole set, not a lookup, so an instrument arriving without a panel
/// or leaving with one pointed at it fails here.
/// Every instrument is *recorded into* first, because a histogram with no
/// data point is absent from a collection rather than empty in it — so the
/// set would be short by one and say nothing about why.
#[tokio::test]
async fn names_every_instrument_the_engine_records() {
    let mut fixture = plain();
    fixture.reorg_detector.queue([Verdict::Reorg {
        first_invalid_block: 98,
        last_invalid_block: 100,
        last_valid_hash: hash_of(b'a', 97),
    }]);

    let _ = fixture.indexer.run_once().await;
    let _ = fixture.indexer.run_once().await;

    assert_eq!(
        instrument_names(&fixture),
        BTreeSet::from(
            [
                "indexer.iterations",
                "indexer.iteration.duration",
                "indexer.processor.duration",
                "indexer.reorg.depth",
                "indexer.blocks.indexed",
                "indexer.cursor.block",
                "indexer.head.block",
                "indexer.lag.blocks",
                "indexer.progress.age",
                "indexer.consecutive_failures",
                "indexer.state",
                "indexer.range.size",
            ]
            .map(str::to_owned)
        )
    );
}

fn points(fixture: &Fixture, name: &str) -> Vec<(Vec<String>, u64)> {
    collected(fixture)
        .iter()
        .flat_map(|batch| batch.scope_metrics().collect::<Vec<_>>())
        .flat_map(|scope| scope.metrics().collect::<Vec<_>>())
        .filter(|metric| metric.name() == name)
        .filter_map(|metric| match metric.data() {
            AggregatedMetrics::U64(data) => Some(data),
            _ => None,
        })
        .flat_map(|data| match data {
            opentelemetry_sdk::metrics::data::MetricData::Gauge(gauge) => gauge
                .data_points()
                .map(|point| {
                    (
                        point
                            .attributes()
                            .map(|attribute| {
                                format!("{}={}", attribute.key, attribute.value.as_str())
                            })
                            .collect(),
                        point.value(),
                    )
                })
                .collect::<Vec<_>>(),
            opentelemetry_sdk::metrics::data::MetricData::Sum(sum) => sum
                .data_points()
                .map(|point| {
                    (
                        point
                            .attributes()
                            .map(|attribute| {
                                format!("{}={}", attribute.key, attribute.value.as_str())
                            })
                            .collect(),
                        point.value(),
                    )
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

/// Reporting 0 would draw a cliff on the graph at every restart and make
/// the lag briefly enormous. Reporting nothing draws a gap, which is what
/// it is.
#[tokio::test]
async fn an_unindexed_chain_publishes_no_cursor_point_rather_than_a_zero() {
    let fixture = plain();

    assert!(points(&fixture, "indexer.cursor.block").is_empty());
    assert!(points(&fixture, "indexer.lag.blocks").is_empty());
    // The ones that always have an answer are still there.
    assert_eq!(points(&fixture, "indexer.consecutive_failures").len(), 1);
}

/// One snapshot per collection, so a panel subtracting the published cursor
/// from the published head agrees with the published lag.
#[tokio::test]
async fn one_collection_cannot_disagree_with_itself() {
    let mut fixture = plain();
    fixture.reorg_detector.settled_through(HEAD);
    let _ = fixture.indexer.run_once().await;

    let cursor = points(&fixture, "indexer.cursor.block")[0].1;
    let head = points(&fixture, "indexer.head.block")[0].1;
    let lag = points(&fixture, "indexer.lag.blocks")[0].1;

    assert_eq!(lag, head - cursor);
}

/// One series per state, so `failed` is alertable without parsing a string.
#[tokio::test]
async fn publishes_one_series_per_state() {
    let mut fixture = plain();
    fixture.processors[0].queue([failed("unfixable")]);
    let _ = fixture.indexer.run_once().await;

    let series = points(&fixture, "indexer.state");
    let hot: Vec<_> = series
        .iter()
        .filter(|(_, value)| *value == 1)
        .flat_map(|(attributes, _)| attributes.clone())
        .filter(|attribute| attribute.starts_with("state="))
        .collect();

    assert_eq!(series.len(), 5, "{series:?}");
    assert_eq!(hot, vec!["state=failed".to_owned()]);
}

#[tokio::test]
async fn the_range_gauge_follows_a_narrowing() {
    let mut fixture = plain();
    fixture.reorg_detector.settled_through(HEAD);
    fixture.processors[0].queue([narrowing("too wide")]);

    assert_eq!(points(&fixture, "indexer.range.size")[0].1, MAX_RANGE);
    let _ = fixture.indexer.run_once().await;

    assert_eq!(points(&fixture, "indexer.range.size")[0].1, MAX_RANGE / 2);
}

#[tokio::test]
async fn counts_the_blocks_it_indexed() {
    let mut fixture = plain();
    fixture.reorg_detector.settled_through(HEAD);

    let _ = fixture.indexer.run_once().await;

    assert_eq!(points(&fixture, "indexer.blocks.indexed")[0].1, MAX_RANGE);
}

#[tokio::test]
async fn counts_an_iteration_under_its_outcome() {
    let mut fixture = plain();
    fixture.processors[0].queue([retry("busy")]);

    let _ = fixture.indexer.run_once().await;

    let counted = points(&fixture, "indexer.iterations");
    assert!(
        counted.iter().any(
            |(attributes, value)| attributes.contains(&"outcome=retry".to_owned()) && *value == 1
        ),
        "{counted:?}"
    );
}

/// Its count is how often, so there is no separate reorg counter.
#[tokio::test]
async fn records_how_deep_a_reorg_reached() {
    let mut fixture = plain();
    fixture.reorg_detector.queue([Verdict::Reorg {
        first_invalid_block: 98,
        last_invalid_block: 100,
        last_valid_hash: hash_of(b'a', 97),
    }]);

    let _ = fixture.indexer.run_once().await;
    fixture.provider.force_flush().expect("a collection");

    let found = fixture
        .exporter
        .get_finished_metrics()
        .expect("batches")
        .iter()
        .flat_map(|batch| batch.scope_metrics().collect::<Vec<_>>())
        .flat_map(|scope| scope.metrics().collect::<Vec<_>>())
        .any(|metric| metric.name() == "indexer.reorg.depth");

    assert!(found);
}
