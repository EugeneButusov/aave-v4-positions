//! The real detector against a chain whose ancestry is derived rather than scripted.

use std::sync::Arc;

use super::in_memory::{
    InMemoryBlockHeaderStore, RecordingBlockHeaderStore, ShufflingBlockHeaderStore,
};
use crate::chain::ChainCall;
use crate::fixtures::{CHAIN_ID, hash_of};
use crate::reorg::harness::{HEAD, cursor_at, forking, harness, over, unprimed};
use crate::reorg::{BlockHeaderStore as _, Error, ReorgDetector, Verdict};

const DEPTH: u64 = 10;

fn reorg(verdict: &Verdict) -> (u64, u64) {
    match verdict {
        Verdict::Reorg {
            first_invalid_block,
            last_invalid_block,
            ..
        } => (*first_invalid_block, *last_invalid_block),
        other => panic!("expected a reorg: {other:?}"),
    }
}

fn unrecoverable(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Unrecoverable { reason } => reason.clone(),
        other => panic!("expected an unrecoverable verdict: {other:?}"),
    }
}

mod finality {
    use super::*;

    #[tokio::test]
    async fn settles_one_finality_depth_below_the_observed_head() {
        let fixture = harness(DEPTH).await;

        let settled = fixture.detector.safe_head(500).await.unwrap();

        assert_eq!(settled, Some(490));
    }

    #[tokio::test]
    async fn settles_nothing_on_a_chain_shorter_than_the_depth() {
        let fixture = harness(DEPTH).await;

        assert_eq!(fixture.detector.safe_head(DEPTH - 1).await.unwrap(), None);
        assert_eq!(fixture.detector.safe_head(DEPTH).await.unwrap(), Some(0));
    }
}

mod inspect {
    use super::*;

    #[tokio::test]
    async fn accepts_a_header_whose_parent_is_the_one_retained() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 1]).await;

        let verdict = fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .unwrap();

        assert_eq!(verdict, Verdict::Continuous);
    }

    #[tokio::test]
    async fn accepts_the_first_block_with_an_empty_window() {
        let fixture = harness(DEPTH).await;

        let verdict = fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .unwrap();

        assert_eq!(verdict, Verdict::Continuous);
    }

    #[tokio::test]
    async fn waves_a_settled_block_through_without_reading_the_window() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 1]).await;
        let before = fixture.chain.header_reads();

        let settled = crate::fixtures::header(HEAD - DEPTH - 5, b'z');
        let verdict = fixture.detector.inspect(&settled).await.unwrap();

        assert_eq!(verdict, Verdict::Continuous);
        assert_eq!(fixture.chain.header_reads(), before, "it read the chain");
    }

    #[tokio::test]
    async fn costs_no_header_read_when_the_chain_simply_extends() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 1]).await;
        let before = fixture.chain.header_reads();

        fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .unwrap();

        assert_eq!(fixture.chain.header_reads(), before);
    }

    #[tokio::test]
    async fn names_the_invalid_range_when_a_parent_hash_does_not_match() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 3, HEAD - 2, HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 3, b'b');

        let verdict = fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .unwrap();

        assert_eq!(reorg(&verdict), (HEAD - 2, HEAD - 1));
    }

    #[tokio::test]
    async fn finds_a_one_block_fork_at_the_tip() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 2, HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 2, b'b');

        let verdict = fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .unwrap();

        assert_eq!(reorg(&verdict), (HEAD - 1, HEAD - 1));
    }

    #[tokio::test]
    async fn catches_a_swap_at_a_height_committed_but_never_cursored() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 2, HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 2, b'b');

        let replacement = fixture.header(HEAD - 1);
        let verdict = fixture.detector.inspect(&replacement).await.unwrap();

        assert_eq!(reorg(&verdict), (HEAD - 1, HEAD - 1));
    }

    #[tokio::test]
    async fn tolerates_re_inspecting_a_block_it_already_holds() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 1, HEAD]).await;

        let verdict = fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .unwrap();

        assert_eq!(verdict, Verdict::Continuous);
    }

    #[tokio::test]
    async fn refuses_when_nothing_is_retained_for_the_block_beneath() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 5]).await;

        let verdict = fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .unwrap();

        assert!(
            unrecoverable(&verdict).contains(&format!("nothing retained for block {}", HEAD - 1)),
            "{verdict:?}"
        );
    }

    #[tokio::test]
    async fn refuses_when_no_retained_header_is_still_canonical() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 2, HEAD - 1]).await;

        fixture.chain.fork_above(HEAD - 2 - DEPTH, b'b');

        let verdict = fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .unwrap();

        assert!(
            unrecoverable(&verdict).contains("is still canonical"),
            "{verdict:?}"
        );
    }

    #[tokio::test]
    async fn reaches_the_same_verdict_over_a_window_read_backwards() {
        let fixture = over(DEPTH, HEAD, Arc::new(ShufflingBlockHeaderStore::new())).await;
        fixture.commit([HEAD - 2, HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 2, b'b');

        let verdict = fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .unwrap();

        assert_eq!(reorg(&verdict), (HEAD - 1, HEAD - 1));
    }

    #[tokio::test]
    async fn surfaces_a_failed_read_rather_than_calling_it_a_fork() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 2, HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 2, b'b');
        fixture.chain.fail_next(1);

        let refusal = fixture
            .detector
            .inspect(&fixture.header(HEAD))
            .await
            .expect_err("the walk needs the chain");

        assert!(matches!(refusal, Error::ChainReadFailed(_)), "{refusal:?}");
    }

    #[tokio::test]
    async fn refuses_a_swap_at_genesis_with_nothing_below_it() {
        let fixture = over(1, 0, Arc::new(InMemoryBlockHeaderStore::new())).await;
        fixture.commit([0]).await;

        let replaced = crate::fixtures::header(0, b'b');
        let verdict = fixture.detector.inspect(&replaced).await.unwrap();

        assert!(
            unrecoverable(&verdict).contains("nothing below block 0"),
            "{verdict:?}"
        );
    }
}

mod bootstrap {
    use super::*;

    #[tokio::test]
    async fn reads_the_head_itself_having_never_been_shown_one() {
        let fixture = unprimed(DEPTH, HEAD);

        fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();

        assert!(
            fixture.chain.calls().contains(&ChainCall::Head),
            "{:?}",
            fixture.chain.calls()
        );
    }

    #[tokio::test]
    async fn surfaces_a_read_that_fails_while_rebuilding_the_run() {
        let fixture = harness(DEPTH).await;
        fixture.chain.fail_next(2);

        let refusal = fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .expect_err("the rebuild needs the chain");

        assert!(matches!(refusal, Error::ChainReadFailed(_)), "{refusal:?}");
    }

    #[tokio::test]
    async fn clears_the_window_when_there_is_no_cursor() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 1]).await;

        let verdict = fixture.detector.bootstrap(None).await.unwrap();

        assert_eq!(verdict, Verdict::Continuous);
        assert!(fixture.retained().await.is_empty());
    }

    #[tokio::test]
    async fn a_still_canonical_cursor_settles_it_in_one_read() {
        let fixture = harness(DEPTH).await;
        let before = fixture.chain.header_reads();

        let verdict = fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();

        assert_eq!(verdict, Verdict::Continuous);
        let depth = usize::try_from(DEPTH).expect("a depth a case chose");
        assert_eq!(fixture.chain.header_reads() - before, 1 + depth);
    }

    #[tokio::test]
    async fn rebuilds_the_window_under_the_resume_point() {
        let fixture = harness(DEPTH).await;

        fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();

        let retained = fixture.retained().await;

        assert_eq!(
            u64::try_from(retained.len()).unwrap(),
            DEPTH + 1,
            "{retained:?}"
        );
        assert_eq!(retained.first(), Some(&(HEAD - 1 - DEPTH)));
        assert_eq!(retained.last(), Some(&(HEAD - 1)));
    }

    // Commit precedes cursor save, so the durable window may be one block ahead.
    #[tokio::test]
    async fn vets_the_window_top_rather_than_the_cursor_behind_it() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 2, HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 2, b'b');

        let verdict = fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 2, b'a')))
            .await
            .unwrap();

        assert_eq!(reorg(&verdict), (HEAD - 1, HEAD - 1));
    }

    #[tokio::test]
    async fn falls_back_to_the_cursor_when_the_window_is_empty() {
        let fixture = harness(DEPTH).await;
        fixture.chain.fork_above(HEAD - 5, b'b');

        let verdict = fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();

        assert!(
            unrecoverable(&verdict).contains("nothing retained"),
            "{verdict:?}"
        );
    }

    #[tokio::test]
    async fn unwinds_a_fork_across_a_restart() {
        let fixture = harness(DEPTH).await;
        fixture
            .commit([HEAD - 4, HEAD - 3, HEAD - 2, HEAD - 1])
            .await;
        fixture.chain.fork_above(HEAD - 3, b'b');

        let verdict = fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();

        assert_eq!(reorg(&verdict), (HEAD - 2, HEAD - 1));
    }

    #[tokio::test]
    async fn leaves_the_window_alone_when_the_resume_point_was_reorged_out() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 2, HEAD - 1]).await;
        let before = fixture.retained().await;
        fixture.chain.fork_above(HEAD - 2, b'b');

        fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();

        assert_eq!(fixture.retained().await, before);
    }

    #[tokio::test]
    async fn stops_rebuilding_where_the_chain_moved_underneath_it() {
        let fixture = forking(DEPTH, 2, HEAD - 5, b'b').await;

        fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();

        let retained = fixture.retained().await;

        assert!(
            (u64::try_from(retained.len()).unwrap()) < DEPTH + 1,
            "the run was rebuilt past the point the chain changed: {retained:?}"
        );
    }
}

mod an_owed_reorg_survives_the_process {
    use super::*;

    #[tokio::test]
    async fn reports_the_same_fork_again_when_nothing_was_applied() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 3, HEAD - 2, HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 3, b'b');

        let first = fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();
        let again = fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();

        assert_eq!(first, again);
        assert_eq!(reorg(&again), (HEAD - 2, HEAD - 1));
    }

    #[tokio::test]
    async fn reports_it_again_when_the_cursor_landed_but_the_rewind_did_not() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 3, HEAD - 2, HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 3, b'b');

        let verdict = fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();
        let (first_invalid, _) = reorg(&verdict);

        let again = fixture
            .detector
            .bootstrap(Some(&cursor_at(first_invalid - 1, b'a')))
            .await
            .unwrap();

        assert_eq!(reorg(&again), reorg(&verdict));
    }

    #[tokio::test]
    async fn stops_reporting_once_the_rewind_also_ran() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 3, HEAD - 2, HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 3, b'b');

        let verdict = fixture
            .detector
            .bootstrap(Some(&cursor_at(HEAD - 1, b'a')))
            .await
            .unwrap();
        let (first_invalid, _) = reorg(&verdict);

        fixture
            .detector
            .rewind_to(Some(first_invalid - 1))
            .await
            .unwrap();

        let after = fixture
            .detector
            .bootstrap(Some(&cursor_at(first_invalid - 1, b'a')))
            .await
            .unwrap();

        assert_eq!(after, Verdict::Continuous);
    }
}

mod the_window_refuses_a_reorg_transition {
    use super::*;

    #[tokio::test]
    async fn a_refused_retention_surfaces_rather_than_being_swallowed() {
        let window = Arc::new(RecordingBlockHeaderStore::new());
        let fixture = over(DEPTH, HEAD, Arc::clone(&window)).await;
        window.refuse_nth_retention(1);

        let refusal = fixture
            .detector
            .commit(&fixture.header(HEAD - 1))
            .await
            .expect_err("header retention was refused");

        assert!(matches!(refusal, Error::RetentionRefused), "{refusal:?}");
        assert!(window.retained().is_empty(), "it retained a header anyway");
    }

    #[tokio::test]
    async fn a_refused_rewind_leaves_the_reorg_owed() {
        let window = Arc::new(RecordingBlockHeaderStore::new());
        let fixture = over(DEPTH, HEAD, Arc::clone(&window)).await;
        fixture.commit([HEAD - 1]).await;
        window.refuse_nth_rewind(1);

        let refusal = fixture
            .detector
            .rewind_to(Some(HEAD - 2))
            .await
            .expect_err("the rewind was refused");

        assert!(matches!(refusal, Error::RewindRefused), "{refusal:?}");
        assert!(window.rewound_to().is_empty());
    }
}

mod retention {
    use super::*;

    #[tokio::test]
    async fn leaves_no_gap_however_the_ranges_land() {
        let fixture = harness(DEPTH).await;

        fixture.commit([HEAD - 40, HEAD - 20, HEAD - 1, HEAD]).await;

        let retained = fixture.retained().await;
        let contiguous = retained.windows(2).all(|pair| pair[1] == pair[0] + 1);

        assert!(contiguous, "{retained:?}");
    }

    // The extra header anchors a fork immediately above the safe head.
    #[tokio::test]
    async fn retains_one_more_than_the_depth() {
        let fixture = harness(DEPTH).await;

        fixture.commit([HEAD]).await;

        assert_eq!(
            u64::try_from(fixture.retained().await.len()).unwrap(),
            DEPTH + 1
        );
    }

    #[tokio::test]
    async fn pushes_the_oldest_out_as_the_tip_advances() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 1]).await;
        let before = fixture.retained().await;

        fixture.commit([HEAD]).await;
        let after = fixture.retained().await;

        assert_eq!(after.len(), before.len());
        assert_eq!(after.first(), Some(&(before[0] + 1)));
    }

    #[tokio::test]
    async fn keeps_only_the_anchor_for_a_settled_range() {
        let fixture = harness(DEPTH).await;

        fixture.commit([HEAD - 500]).await;

        assert_eq!(fixture.retained().await, vec![HEAD - 500]);
    }

    #[tokio::test]
    async fn fills_the_run_for_the_range_that_lands_on_the_boundary() {
        let fixture = harness(DEPTH).await;

        fixture.commit([HEAD - DEPTH]).await;

        assert_eq!(
            u64::try_from(fixture.retained().await.len()).unwrap(),
            DEPTH + 1
        );
    }

    #[tokio::test]
    async fn tops_the_window_back_up_after_a_rewind() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD]).await;

        fixture.detector.rewind_to(Some(HEAD - 5)).await.unwrap();
        fixture.commit([HEAD - 4]).await;

        assert_eq!(
            u64::try_from(fixture.retained().await.len()).unwrap(),
            DEPTH + 1
        );
    }

    #[tokio::test]
    async fn keeps_the_block_a_rewind_landed_on() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD]).await;

        fixture.detector.rewind_to(Some(HEAD - 5)).await.unwrap();

        assert_eq!(fixture.retained().await.last(), Some(&(HEAD - 5)));
    }

    #[tokio::test]
    async fn clears_the_window_when_rewound_below_everything() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD]).await;

        fixture.detector.rewind_to(None).await.unwrap();

        assert!(fixture.retained().await.is_empty());
    }

    #[tokio::test]
    async fn restarts_the_window_when_a_header_does_not_extend_the_run() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 2, HEAD - 1]).await;

        let stacked = crate::fixtures::header(HEAD, b'b');
        fixture.detector.commit(&stacked).await.unwrap();

        let retained = fixture.retained().await;
        let window = fixture.window.load(CHAIN_ID).await.unwrap();

        assert!(
            window
                .iter()
                .all(|held| held.hash == hash_of(b'b', held.number)),
            "a branch-a header survived the restart: {retained:?}"
        );
    }

    #[tokio::test]
    async fn replaces_a_height_re_committed_on_another_branch() {
        let fixture = harness(DEPTH).await;
        fixture.commit([HEAD - 1]).await;
        fixture.chain.fork_above(HEAD - 2, b'b');

        fixture.commit([HEAD - 1]).await;
        let window = fixture.window.load(CHAIN_ID).await.unwrap();

        let at_height: Vec<_> = window
            .iter()
            .filter(|held| held.number == HEAD - 1)
            .collect();

        assert_eq!(at_height.len(), 1, "{window:?}");
        assert_eq!(at_height[0].hash, hash_of(b'b', HEAD - 1));
    }
}
