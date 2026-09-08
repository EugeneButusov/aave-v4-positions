-- One wallet's position in one reserve, at event grain.
--
-- **The grain is the change.** The old table summed every delta for a position
-- into one row, which answers "what is this now" and cannot answer "what was
-- this at T" — there was no time coordinate to cut on, so `asOf` valued today's
-- shares against a past interest index and returned a number that was never
-- true at any block.
--
-- **The cost was measured before it was paid.** Event grain is 33,139 rows where
-- the summed table was 7,274 — 4.6x, about 72k rows a year at the current rate.
-- A page reads far less than that: median 2 events per position, p99 40, the
-- busiest position in the whole fold 297, and the busiest wallet's entire page
-- 823 rows across 8 reserves. `(chain_id, user, spoke, reserve_id)` still leads
-- the sorting key, so a page is a binary search into a few hundred contiguous
-- rows rather than a scan, exactly as before.
--
-- `block_timestamp` last in the key, so the wallet prefix keeps doing the
-- seeking and a cut is a range inside the run it lands on. Partitioned by
-- `chain_id` and not by month: the prefix already prunes, and `hub_asset_deltas`
-- needs month partitions only because its reads have no key to prune by.
--
-- Still `SummingMergeTree`. Two deltas for one position in one log — a
-- liquidation crediting collateral and charging debt to the same wallet — share
-- the whole key and merge, which is correct. A retraction is the same log with
-- `sign = -1` and the same key, so a reorg's pair sums to zero rather than
-- needing collapse, and both halves fall on the same side of any cut.
CREATE TABLE IF NOT EXISTS user_positions
(
    chain_id            UInt32,
    user                String,
    spoke               String,
    reserve_id          UInt256,
    -- The block's own instant, carried from `spoke_events`. This is the whole
    -- point of the rebuild: without it a fold can only answer for now.
    block_timestamp     DateTime('UTC'),
    supplied_shares     Int256,
    drawn_shares        Int256,
    premium_shares      Int256,
    premium_offset_ray  Int256,
    net_supplied_amount Int256,
    net_borrowed_amount Int256,
    events              Int64
)
ENGINE = SummingMergeTree
PARTITION BY chain_id
ORDER BY (chain_id, user, spoke, reserve_id, block_timestamp);
