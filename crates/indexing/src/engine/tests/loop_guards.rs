use crate::chain::ChainCall;
use crate::engine::harness::{HEAD, START, Setup, failed, fixture, options, plain, reason};
use crate::engine::status::IndexerState;
use crate::engine::{Iteration, Options};
use crate::fixtures::{CHAIN_ID, OTHER_CHAIN};

#[tokio::test]
async fn idles_when_the_start_block_is_ahead_of_the_chain() {
    let mut fixture = fixture(Setup {
        head: START - 1,
        ..Setup::default()
    });

    let result = fixture.indexer.run_once().await;

    assert_eq!(result, Iteration::Idle);
    assert!(fixture.processors[0].calls().is_empty());
}

#[tokio::test]
async fn idles_once_it_has_caught_up() {
    let mut fixture = fixture(Setup {
        head: START,
        ..Setup::default()
    });

    let _ = fixture.indexer.run_once().await;
    let caught_up = fixture.indexer.run_once().await;

    assert_eq!(caught_up, Iteration::Idle);
}

/// Idling counts as progress: being current is the healthy steady state,
/// and a quiet chain must not trip the stall signal.
#[tokio::test]
async fn an_idle_iteration_is_progress_for_the_stall_clock() {
    let mut fixture = fixture(Setup {
        head: START - 1,
        ..Setup::default()
    });

    let _ = fixture.indexer.run_once().await;

    let snapshot = fixture.status.snapshot();
    assert_eq!(snapshot.state, IndexerState::Running);
    assert_eq!(snapshot.consecutive_failures, 0);
}

/// A failover to a lagging node is otherwise indistinguishable from a
/// reorg, and it can invert a range into `from > to`.
#[tokio::test]
async fn a_backwards_head_is_clamped_to_the_high_mark() {
    let mut fixture = fixture(Setup {
        head: START + 10,
        ..Setup::default()
    });
    fixture.reorg_detector.settled_through(HEAD);

    let _ = fixture.indexer.run_once().await;
    fixture.chain_client.set_head(START);
    let _ = fixture.indexer.run_once().await;

    assert_eq!(fixture.status.snapshot().head, Some(START + 10));
}

#[tokio::test]
async fn stops_on_a_chain_id_mismatch() {
    let mut fixture = plain();
    fixture.chain_client.set_chain_id(OTHER_CHAIN);

    let result = fixture.indexer.run_once().await;

    assert_eq!(
        reason(&result),
        format!(
            "chain id mismatch: configured {CHAIN_ID}, provider reports {}",
            OTHER_CHAIN
        )
    );
}

#[tokio::test]
async fn verifies_the_chain_id_once() {
    let mut fixture = plain();

    let _ = fixture.indexer.run_once().await;
    let _ = fixture.indexer.run_once().await;

    let asked = fixture
        .chain_client
        .calls()
        .iter()
        .filter(|call| matches!(call, ChainCall::ChainId))
        .count();

    assert_eq!(asked, 1);
}

/// A node that cannot be reached is a retry, not a mismatch. Treating it as
/// terminal would make a provider outage a crash loop.
#[tokio::test]
async fn an_unreachable_chain_retries_rather_than_stopping() {
    let mut fixture = plain();
    fixture.chain_client.fail_next(1);

    let result = fixture.indexer.run_once().await;

    assert!(matches!(result, Iteration::Retry { .. }), "{result:?}");
    assert!(reason(&result).starts_with("unhandled:"));
}

#[tokio::test]
async fn a_processor_is_told_that_shutdown_began() {
    let mut fixture = plain();
    fixture.cancel.cancel();

    let _ = fixture.indexer.run_once().await;

    assert!(
        fixture.processors[0]
            .calls()
            .iter()
            .all(|call| call.cancelled)
    );
}

#[tokio::test]
async fn the_loop_stops_when_cancelled() {
    let mut fixture = plain();
    fixture.cancel.cancel();

    fixture.indexer.run().await;

    assert_eq!(fixture.status.snapshot().state, IndexerState::Stopped);
}

/// Terminal, and the state says so rather than reading `stopped` — which is
/// what the readiness check keys on.
#[tokio::test]
async fn a_terminal_failure_leaves_the_loop_failed_rather_than_stopped() {
    let mut fixture = plain();
    fixture.processors[0].queue([failed("unfixable")]);

    fixture.indexer.run().await;

    assert_eq!(fixture.status.snapshot().state, IndexerState::Failed);
}

/// Without this a quiet loop would sit out its whole poll interval before
/// noticing it had been asked to stop.
#[tokio::test(start_paused = true)]
async fn shutdown_does_not_wait_out_the_poll_interval() {
    let mut fixture = fixture(Setup {
        head: START - 1,
        options: Options {
            poll_interval: std::time::Duration::from_secs(600),
            ..options()
        },
        ..Setup::default()
    });

    let started = tokio::time::Instant::now();
    let cancel = fixture.cancel.clone();
    let stopper = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        cancel.cancel();
    });

    fixture.indexer.run().await;
    stopper.await.unwrap();

    assert!(
        started.elapsed() < std::time::Duration::from_secs(600),
        "{:?}",
        started.elapsed()
    );
}

/// 500ms doubling to a 30s ceiling, and the ends of it: no wait at all
/// after progress, the poll interval after idling.
#[test]
fn the_backoff_doubles_to_a_ceiling() {
    use crate::engine::indexer::backoff;

    let waits: Vec<_> = (1..=8).map(backoff).collect();

    assert_eq!(
        waits,
        vec![
            std::time::Duration::from_millis(500),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(4),
            std::time::Duration::from_secs(8),
            std::time::Duration::from_secs(16),
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(30),
        ]
    );
    // The count the loop has before its first failure is recorded.
    assert_eq!(backoff(0), std::time::Duration::from_millis(500));
}

/// And the loop actually waits it, which the formula on its own does not
/// say.
#[tokio::test(start_paused = true)]
async fn the_loop_waits_its_backoff_between_attempts() {
    let mut fixture = plain();
    fixture.processors[0].refuse_with("provider down");

    let cancel = fixture.cancel.clone();
    let stopper = tokio::spawn(async move {
        // Past the first backoff and into the second, so at least one was
        // waited out in full.
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        cancel.cancel();
    });

    let started = tokio::time::Instant::now();
    fixture.indexer.run().await;
    stopper.await.unwrap();

    assert!(
        started.elapsed() >= std::time::Duration::from_millis(500),
        "it did not wait: {:?}",
        started.elapsed()
    );
    assert!(
        fixture.processors[0].calls().len() >= 2,
        "it stopped after one attempt"
    );
}
