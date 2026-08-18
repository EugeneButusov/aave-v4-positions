-- The reserve registry: what a Spoke's `reserveId` actually refers to.
--
-- `reserveId` is a per-Spoke index, not a protocol-wide asset id (§1), so it
-- means nothing on its own. `AddReserve(reserveId, assetId, hub)` is the only
-- event that gives it one, and joining a position to Hub state is impossible
-- without it — which is why valuation waited for this table rather than the
-- other way round.
--
-- **The rows are already in the ledger.** `AddReserve` has been ingested since
-- the events package existed, with no projection, precisely so that the
-- increment that first needed the mapping could read stored data instead of
-- re-indexing. A materialized view still does not see rows written before it,
-- so this needs a replay over the range — the backfill command re-dispatches
-- the same revert-then-append without touching the cursor.
--
-- Event grain and collapsing rather than one row per reserve. A listing is
-- latest-wins like any other config fact, and event grain is the only grain at
-- which one can be retracted (the same argument as `user_position_flags`).
-- Fourteen rows over all Main Spoke history.
CREATE TABLE IF NOT EXISTS spoke_reserves
(
    chain_id     UInt32,
    spoke        String,
    reserve_id   UInt256,
    block_number UInt64,
    log_index    UInt32,
    version      UInt64,
    -- The Hub's own id for the asset, and the Hub holding it. Both are needed:
    -- asset ids are Hub-scoped, so `assetId` alone does not identify anything.
    asset_id     UInt256,
    hub          String,
    sign         Int8
)
ENGINE = VersionedCollapsingMergeTree(sign, version)
PARTITION BY chain_id
ORDER BY (chain_id, spoke, reserve_id, block_number, log_index);
