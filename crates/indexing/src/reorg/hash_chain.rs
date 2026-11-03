use std::sync::Arc;

use tracing::warn;
use units::ChainId;

use crate::chain::{BlockHeader, ChainClient};
use crate::cursor::Cursor;
use crate::reorg::{BlockHeaderStore, Error, ReorgDetector, Verdict};

/// Detects forks against a durable, contiguous window of processed headers.
///
/// Canonical reads may verify the window but never replace its evidence. A
/// reported fork therefore remains owed until rewind succeeds. Forks at or
/// below the safe head are unrecoverable because those ranges were not inspected.
pub struct HashChainReorgDetector {
    chain_id: ChainId,
    finality_depth: u64,

    chain_client: Arc<dyn ChainClient>,
    block_header_store: Box<dyn BlockHeaderStore>,
}

impl HashChainReorgDetector {
    #[must_use]
    pub fn new(
        chain_id: ChainId,
        finality_depth: u64,
        chain_client: Arc<dyn ChainClient>,
        block_header_store: Box<dyn BlockHeaderStore>,
    ) -> Self {
        Self {
            chain_id,
            finality_depth,
            chain_client,
            block_header_store,
        }
    }
}

#[async_trait::async_trait]
impl ReorgDetector for HashChainReorgDetector {
    async fn bootstrap(&self, cursor: Option<&Cursor>) -> Result<Verdict, Error> {
        let Some(cursor) = cursor else {
            self.block_header_store
                .truncate(self.chain_id, None)
                .await?;

            return Ok(Verdict::Continuous);
        };

        let retained = self.retained_window().await?;
        let ours = retained.last().map_or(*cursor, |top| Cursor {
            chain_id: self.chain_id,
            last_block: top.number,
            last_hash: top.hash,
        });

        let canonical = self.chain_client.block_header(ours.last_block).await?;

        if canonical.hash != ours.last_hash {
            return self
                .locate_fork(&retained, ours.last_block.checked_sub(1), ours.last_block)
                .await;
        }

        let head = self.chain_client.head_block_number().await?;
        self.commit(&canonical, self.safe_head(head)).await?;

        Ok(Verdict::Continuous)
    }

    fn safe_head(&self, observed_head: u64) -> Option<u64> {
        observed_head.checked_sub(self.finality_depth)
    }

    async fn inspect(
        &self,
        header: &BlockHeader,
        safe_head: Option<u64>,
    ) -> Result<Verdict, Error> {
        // Settled ranges retain only their top, so there is no ancestry to inspect.
        if let Some(settled) = safe_head
            && header.number <= settled
        {
            return Ok(Verdict::Continuous);
        }

        let retained = self.retained_window().await?;

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

    async fn commit(&self, header: &BlockHeader, safe_head: Option<u64>) -> Result<(), Error> {
        let retained = self.retained_window().await?;

        // Settled ranges bypass `inspect`; catch a finality violation before restart.
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
            self.block_header_store
                .truncate(self.chain_id, None)
                .await?;
        }

        let floor = header.number.saturating_sub(self.finality_depth);
        self.block_header_store
            .append(self.chain_id, header, floor)
            .await?;

        let bottom = match retained.first() {
            Some(oldest) if continues => oldest.number.max(floor),
            _ => header.number,
        };

        if bottom > floor {
            self.fill_run_below(header, safe_head).await?;
        }

        Ok(())
    }

    async fn rewind_to(&self, last_valid_block: Option<u64>) -> Result<(), Error> {
        self.block_header_store
            .truncate(self.chain_id, last_valid_block)
            .await
    }
}

impl HashChainReorgDetector {
    async fn retained_window(&self) -> Result<Vec<BlockHeader>, Error> {
        let mut window = self.block_header_store.load(self.chain_id).await?;
        window.sort_unstable_by_key(|held| held.number);

        Ok(window)
    }

    async fn locate_fork(
        &self,
        retained: &[BlockHeader],
        walk_from: Option<u64>,
        last_invalid_block: u64,
    ) -> Result<Verdict, Error> {
        let Some(ancestor) = self.find_ancestor(retained, walk_from).await? else {
            return Ok(Verdict::Unrecoverable {
                reason: describe_search(retained, walk_from),
            });
        };

        Ok(Verdict::Reorg {
            first_invalid_block: ancestor.number + 1,
            last_invalid_block,
            last_valid_hash: ancestor.hash,
        })
    }

    /// Finds the highest retained header still canonical at or below `from`.
    /// Never fills gaps from the winning branch, which would under-report a fork.
    async fn find_ancestor(
        &self,
        retained: &[BlockHeader],
        from: Option<u64>,
    ) -> Result<Option<BlockHeader>, Error> {
        let Some(from) = from else {
            return Ok(None);
        };

        for candidate in retained.iter().rev().filter(|held| held.number <= from) {
            let canonical = self.chain_client.block_header(candidate.number).await?;

            if canonical.hash == candidate.hash {
                return Ok(Some(*candidate));
            }
        }

        Ok(None)
    }

    /// Fills predecessors to the retention floor, verifying every parent link.
    /// Settled anchors are skipped to avoid a depth-sized walk per backfill range.
    async fn fill_run_below(
        &self,
        anchor: &BlockHeader,
        safe_head: Option<u64>,
    ) -> Result<(), Error> {
        if let Some(settled) = safe_head
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

    async fn retain_parent_of(
        &self,
        child: &BlockHeader,
        floor: u64,
    ) -> Result<Option<BlockHeader>, Error> {
        let parent = self.chain_client.block_header(child.number - 1).await?;

        if parent.hash != child.parent_hash {
            return Ok(None);
        }

        self.block_header_store
            .append(self.chain_id, &parent, floor)
            .await?;

        Ok(Some(parent))
    }
}

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

fn at(retained: &[BlockHeader], number: u64) -> Option<&BlockHeader> {
    retained.iter().find(|held| held.number == number)
}

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
