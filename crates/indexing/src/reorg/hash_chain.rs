use std::sync::{Arc, Mutex};

use tracing::warn;

use crate::chain::{BlockHeader, ChainClient};
use crate::cursor::Cursor;
use crate::reorg::{BlockHeaderStore, Error, ReorgDetector, Verdict};

/// Finds forks by keeping the headers it committed and checking each new block's
/// `parent_hash` against them. The steady state costs nothing — that hash is
/// already in the header the caller fetched — so only a mismatch spends a call,
/// and only for as deep as the fork runs.
///
/// Three properties hold it together, each argued where it is enforced:
///
/// - **The window is what was processed, not what the chain says**, so a header
///   is retained only behind a verified `parent_hash` link ([`fill_run_below`]).
/// - **It is one contiguous hash chain**, because [`commit`] restarts the window
///   rather than leave a gap the walk could not cross.
/// - **A reported fork stays owed until applied**, with nothing written down for
///   it: an unfinished unwind leaves the window holding the abandoned branch
///   above the cursor, and [`bootstrap`] re-reports until it sticks.
///
/// A fork at or below the safe head is reported, never repaired — those blocks
/// went out as settled ranges and were never inspected.
///
/// [`fill_run_below`]: Self::fill_run_below
/// [`commit`]: Self::commit
/// [`bootstrap`]: Self::bootstrap
pub struct HashChainReorgDetector {
    chain_id: u32,
    finality_depth: u64,

    /// Shared rather than owned: the loop reads the head through the same
    /// client, and two of them over one provider list diverge the moment one
    /// fails over and the other does not.
    chain: Arc<dyn ChainClient>,
    window: Box<dyn BlockHeaderStore>,

    /// The head last shown to [`safe_head`](Self::safe_head), so nothing
    /// re-reads it mid-iteration.
    observed_head: Mutex<Option<u64>>,
}

impl HashChainReorgDetector {
    #[must_use]
    pub fn new(
        chain_id: u32,
        finality_depth: u64,
        chain: Arc<dyn ChainClient>,
        window: Box<dyn BlockHeaderStore>,
    ) -> Self {
        Self {
            chain_id,
            finality_depth,
            chain,
            window,
            observed_head: Mutex::new(None),
        }
    }

    /// Poisoning is ignored rather than refused: a panic while this was held has
    /// already failed whatever was running, and a cached head is an optimisation
    /// — the next call reads one.
    fn remembered_head(&self) -> Option<u64> {
        *self
            .observed_head
            .lock()
            .unwrap_or_else(|held| held.into_inner())
    }

    fn remember(&self, head: u64) {
        *self
            .observed_head
            .lock()
            .unwrap_or_else(|held| held.into_inner()) = Some(head);
    }

    /// Only [`bootstrap`](Self::bootstrap), running before any head is read, has
    /// to spend a call here.
    async fn current_safe_head(&self) -> Result<Option<u64>, Error> {
        let head = match self.remembered_head() {
            Some(head) => head,
            None => self.chain.head_block_number().await?,
        };

        self.safe_head(head).await
    }

    /// Ascending. Read positionally below, and the port promises no order, so
    /// leaning on the store's would under-report a fork.
    async fn retained(&self) -> Result<Vec<BlockHeader>, Error> {
        let mut window = self.window.load(self.chain_id).await?;
        window.sort_unstable_by_key(|held| held.number);

        Ok(window)
    }

    /// Pulls a header's predecessors in, following `parent_hash` down and
    /// checking every link — a resume or a window restart leaves one header
    /// where a full run belongs, and one header can place no fork beyond the
    /// shallowest.
    ///
    /// **Skipped below the boundary**, or a backfill would pull a full depth per
    /// dispatched range and cost more calls than the backfill itself.
    async fn fill_run_below(&self, anchor: &BlockHeader) -> Result<(), Error> {
        if let Some(settled) = self.current_safe_head().await?
            && anchor.number < settled
        {
            return Ok(());
        }

        let floor = anchor.number.saturating_sub(self.finality_depth);
        let mut child = *anchor;

        while child.number > floor {
            let Some(parent) = self.retain_parent_of(&child, floor).await? else {
                return Ok(());
            };
            child = parent;
        }

        Ok(())
    }

    /// `None` when the chain no longer agrees this is the parent. Splicing that
    /// header in would leave a window that looks like a chain and is not, so the
    /// walk would later stop at a block nothing here processed and under-report.
    async fn retain_parent_of(
        &self,
        child: &BlockHeader,
        floor: u64,
    ) -> Result<Option<BlockHeader>, Error> {
        // `child.number > floor >= 0`, so there is a block below it.
        let parent = self.chain.block_header(child.number - 1).await?;

        if parent.hash != child.parent_hash {
            return Ok(None);
        }

        self.window.append(self.chain_id, &parent, floor).await?;

        Ok(Some(parent))
    }

    /// Turns a failed ancestry check into a verdict. `implied_last_invalid` is
    /// the highest block known to be on the abandoned branch; every caller has
    /// it before walking.
    async fn locate_fork(
        &self,
        retained: &[BlockHeader],
        walk_from: Option<u64>,
        implied_last_invalid: u64,
    ) -> Result<Verdict, Error> {
        let Some(ancestor) = self.find_ancestor(retained, walk_from).await? else {
            return Ok(Verdict::Unrecoverable {
                reason: describe_search(retained, walk_from),
            });
        };

        Ok(Verdict::Reorg {
            first_invalid_block: ancestor.number + 1,
            last_invalid_block: implied_last_invalid,
            last_valid_hash: ancestor.hash,
        })
    }

    /// The highest header retained at or below `from` that the chain still
    /// agrees with — where the two branches meet. Descending one at a time keeps
    /// the usual one-block reorg to a single call.
    ///
    /// **Nothing here fills a gap from the chain**, and [`commit`](Self::commit)
    /// is what makes sure there is none: by the time this runs the chain has
    /// forked, so a header read by height is the branch that *won* — it would
    /// match itself, be named the ancestor, and under-report the fork.
    ///
    /// A provider failover mid-walk could answer from the old branch and place
    /// the fork too high. Unmitigated, and it is why steady-state traffic stays
    /// on one provider rather than being balanced across them.
    async fn find_ancestor(
        &self,
        retained: &[BlockHeader],
        from: Option<u64>,
    ) -> Result<Option<BlockHeader>, Error> {
        let Some(from) = from else {
            return Ok(None);
        };

        for candidate in retained.iter().rev().filter(|held| held.number <= from) {
            let canonical = self.chain.block_header(candidate.number).await?;

            if canonical.hash == candidate.hash {
                return Ok(Some(*candidate));
            }
        }

        Ok(None)
    }
}

#[async_trait::async_trait]
impl ReorgDetector for HashChainReorgDetector {
    /// Vets the resume point against the chain and rebuilds the window under it.
    /// One header read and no walk in the ordinary case, since a block hash
    /// commits to its whole ancestry.
    ///
    /// **The window's top is what gets vetted**, the cursor being the fallback
    /// for an empty window. The top is the highest block processed and never
    /// behind the cursor, because the loop commits before saving it and rewinds
    /// only after; vetting the cursor would call both of those gaps continuous,
    /// and nothing inspects a block that has since fallen below the safe head.
    ///
    /// A mismatch leaves the window alone: headers read by height now describe
    /// the branch that won, and recording them would erase the evidence of the
    /// one that lost.
    async fn bootstrap(&self, cursor: Option<&Cursor>) -> Result<Verdict, Error> {
        let Some(cursor) = cursor else {
            self.window.truncate(self.chain_id, None).await?;

            return Ok(Verdict::Continuous);
        };

        let retained = self.retained().await?;
        let ours = retained
            .last()
            .map_or((cursor.last_block, cursor.last_hash), |top| {
                (top.number, top.hash)
            });

        let canonical = self.chain.block_header(ours.0).await?;

        if canonical.hash != ours.1 {
            return self
                .locate_fork(&retained, ours.0.checked_sub(1), ours.0)
                .await;
        }

        self.commit(&canonical).await?;

        Ok(Verdict::Continuous)
    }

    /// `None` below the depth, so a chain shorter than it treats everything as
    /// unsettled.
    async fn safe_head(&self, observed_head: u64) -> Result<Option<u64>, Error> {
        self.remember(observed_head);

        Ok(observed_head.checked_sub(self.finality_depth))
    }

    /// Three ways a header is not a plain extension.
    ///
    /// The window may already hold this height, since a rejected cursor save has
    /// the loop replay the block — and a *different* block there was swapped
    /// underneath with its events already folded, which the parent check would
    /// wave through. It may hold nothing for the block beneath, which the loop
    /// cannot cause, so the window and the cursor have drifted and there is no
    /// ancestry left to bound a fork with. Otherwise the parent link decides,
    /// and a mismatch walks from `number - 2`: the parent height is already
    /// disproved, so re-reading it would buy nothing.
    ///
    /// An empty window means nothing was processed — a cold start, with no
    /// ancestry to contradict.
    async fn inspect(&self, header: &BlockHeader) -> Result<Verdict, Error> {
        // Settled, so nothing can reach it — and nothing could check it if it
        // could: it arrives as the top of a range whose other blocks were never
        // fetched, and the window restarts on each such top. Deciding that here
        // rather than in the loop is the point; the loop only chooses how wide
        // to dispatch, and asks about every header it commits.
        if let Some(settled) = self.current_safe_head().await?
            && header.number <= settled
        {
            return Ok(Verdict::Continuous);
        }

        let retained = self.retained().await?;

        if retained.is_empty() {
            return Ok(Verdict::Continuous);
        }

        if let Some(existing) = at(&retained, header.number)
            && existing.hash != header.hash
        {
            return self
                .locate_fork(&retained, header.number.checked_sub(1), header.number)
                .await;
        }

        let Some(below) = header.number.checked_sub(1) else {
            return Ok(Verdict::Continuous);
        };

        let Some(parent) = at(&retained, below) else {
            return Ok(Verdict::Unrecoverable {
                reason: format!(
                    "nothing retained for block {below}, so block {} has no ancestry to check",
                    header.number
                ),
            });
        };

        if parent.hash == header.parent_hash {
            return Ok(Verdict::Continuous);
        }

        self.locate_fork(&retained, header.number.checked_sub(2), below)
            .await
    }

    /// Records a header as processed, keeping the window one contiguous hash
    /// chain.
    ///
    /// A header that does not continue the run restarts it: a run on the far
    /// side of a hole cannot be walked back through, so it is evidence of
    /// nothing. That is the ordinary case while catching up, where each settled
    /// range top lands clear of the last.
    ///
    /// Retains `finality_depth + 1` headers, the floor inclusive. The extra one
    /// is load-bearing: the deepest recoverable fork starts at `safe_head + 1`,
    /// and placing it needs its parent at exactly the safe head. The window is
    /// topped back up whenever the run falls short of that floor, which covers
    /// both ways it can — a restart leaving this header alone, a deep rewind
    /// stripping it.
    async fn commit(&self, header: &BlockHeader) -> Result<(), Error> {
        let retained = self.retained().await?;

        // Where nothing else would notice: `inspect` covers the tip, but a
        // settled range is dispatched without it, so a one-block range could
        // otherwise seat a header on a parent never compared against it. A
        // break here means the chain changed at or below the safe head, which
        // finality says cannot happen.
        if let Some(below) = header.number.checked_sub(1)
            && let Some(parent) = at(&retained, below)
            && parent.hash != header.parent_hash
        {
            warn!(
                block = header.number,
                retained = parent.number,
                "block does not extend the retained one below it; something changed at or \
                 below the safe head, so the window is being restarted"
            );
        }

        let continues = continues_the_run(&retained, header);

        if !continues {
            self.window.truncate(self.chain_id, None).await?;
        }

        let floor = header.number.saturating_sub(self.finality_depth);
        self.window.append(self.chain_id, header, floor).await?;

        let bottom = match retained.first() {
            Some(oldest) if continues => oldest.number.max(floor),
            _ => header.number,
        };

        if bottom > floor {
            self.fill_run_below(header).await?;
        }

        Ok(())
    }

    async fn rewind_to(&self, last_valid_block: Option<u64>) -> Result<(), Error> {
        self.window.truncate(self.chain_id, last_valid_block).await
    }
}

fn at(retained: &[BlockHeader], number: u64) -> Option<&BlockHeader> {
    retained.iter().find(|held| held.number == number)
}

/// Whether `header` leaves the window one unbroken **hash** chain, not merely an
/// unbroken run of numbers: adjacent numbers over mismatched hashes are two
/// branches stacked on each other, worse than a hole because they still look
/// walkable. The number only decides where the block below is not retained.
fn continues_the_run(retained: &[BlockHeader], header: &BlockHeader) -> bool {
    let (Some(oldest), Some(newest)) = (retained.first(), retained.last()) else {
        return true;
    };

    if let Some(below) = header.number.checked_sub(1)
        && let Some(parent) = at(retained, below)
    {
        return parent.hash == header.parent_hash;
    }

    header.number + 1 >= oldest.number && header.number <= newest.number + 1
}

/// Names which way the walk ran out.
fn describe_search(retained: &[BlockHeader], walk_from: Option<u64>) -> String {
    let Some(walk_from) = walk_from else {
        return "nothing below block 0 to compare against".to_owned();
    };

    match retained.first() {
        Some(oldest) if oldest.number <= walk_from => format!(
            "no retained header between {} and {walk_from} is still canonical",
            oldest.number
        ),
        _ => format!("nothing retained at or below block {walk_from} to compare against"),
    }
}
