use crate::engine::harness::{START, Setup, failed, fixture, plain, reason, retry};

#[tokio::test]
async fn hands_the_same_range_to_every_processor_in_registration_order() {
    let mut fixture = fixture(Setup {
        processors: 3,
        ..Setup::default()
    });

    let _ = fixture.indexer.run_once().await;

    for processor in &fixture.processors {
        assert_eq!(processor.ranges(), vec![(START, START)]);
    }
    assert_eq!(
        fixture
            .trace
            .recorded()
            .iter()
            .filter(|entry| entry.contains("block-range"))
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            format!("p1.block-range({START},{START})"),
            format!("p2.block-range({START},{START})"),
            format!("p3.block-range({START},{START})"),
        ]
    );
}

#[tokio::test]
async fn stops_at_the_first_processor_that_does_not_accept_the_range() {
    let mut fixture = fixture(Setup {
        processors: 3,
        ..Setup::default()
    });
    fixture.processors[1].queue([retry("busy")]);

    let _ = fixture.indexer.run_once().await;

    assert_eq!(fixture.processors[0].ranges().len(), 1);
    assert_eq!(fixture.processors[1].ranges().len(), 1);
    assert!(fixture.processors[2].ranges().is_empty(), "it ran anyway");
}

#[tokio::test]
async fn names_the_processor_that_refused() {
    let mut fixture = plain();
    fixture.processors[0].queue([failed("decoder is wrong")]);

    let result = fixture.indexer.run_once().await;

    assert_eq!(reason(&result), "p1: decoder is wrong");
}

/// Nothing is committed and nothing is saved when a processor holds the
/// range, which is what makes the cursor the commit point.
#[tokio::test]
async fn commits_nothing_when_a_processor_holds_the_range() {
    let mut fixture = plain();
    fixture.processors[0].queue([retry("busy")]);

    let _ = fixture.indexer.run_once().await;

    assert!(fixture.reorg_detector.committed().is_empty());
    assert!(fixture.cursor_store.saved().is_empty());
}
