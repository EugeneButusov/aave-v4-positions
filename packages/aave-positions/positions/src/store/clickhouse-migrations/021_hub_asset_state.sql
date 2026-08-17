-- The latest-wins half of the Hub asset fold.
--
-- **No engine holds a latest-wins fact pre-aggregated under retraction** — the
-- three candidates were measured and ruled out for `user_position_flags`, and
-- nothing about the Hub changes that: `argMaxState` has no operation that
-- removes a contribution, a `ReplacingMergeTree` tombstone deletes the key
-- rather than the generation, and `argMaxIf(…, sign = 1)` returns the retracted
-- row because a retraction is a *pair* whose `+1` twin still carries `sign = 1`.
--
-- So these stay at **event grain**, where a retraction is an ordinary
-- collapsing pair, and the read view resolves them with `argMax` over whatever
-- the collapse leaves standing.
--
-- Three independent groups in one table rather than three tables. They share
-- the grain, the key and the collapse; only the columns differ, and each is
-- resolved by its own `argMaxIf(col, …, col IS NOT NULL)` so a row written by
-- `UpdateAsset` never overwrites what `AddAsset` set.
CREATE TABLE IF NOT EXISTS hub_asset_state
(
    chain_id           UInt32,
    hub                String,
    asset_id           UInt256,

    -- Chain order, and the tiebreak the read view argMaxes on. **Never
    -- `version`**: version orders *dispatches*, and the loop re-dispatches an
    -- earlier range whenever a later processor asks to retry, so version
    -- ordering reads a stale value. Measured on the collateral flag.
    block_number       UInt64,
    log_index          UInt32,
    -- Copied from the source row: half the pairing key, and what makes the
    -- retraction collapse.
    version            UInt64,

    -- From UpdateAsset. The interest checkpoint (§5.3) — the emitted index is
    -- the *settled* one, because `accrue()` writes `asset.drawnIndex` before
    -- `updateDrawnRate` reads it to emit.
    --
    -- `realized_fees` is here rather than in `hub_assets` because it is not
    -- additive: `accrue()` adds the unrealized part and `_mintFeeShares` sets it
    -- to zero. Latest-wins is nonetheless exact, and for a reason verified in
    -- Hub.sol rather than assumed — **all 14 functions that call `accrue()` also
    -- call `updateDrawnRate()`**, so every mutation of `realizedFees` is
    -- followed by an `UpdateAsset` in the same transaction carrying the settled
    -- value. `MintFeeShares` zeroing it is emitted first and superseded by that
    -- `UpdateAsset` at a higher `log_index`.
    drawn_index        Nullable(UInt256),
    drawn_rate         Nullable(UInt256),
    realized_fees      Nullable(UInt256),
    -- `UpdateAsset` carries no timestamp; `accrue()` sets
    -- `lastUpdateTimestamp = block.timestamp`, so the block's is the
    -- checkpoint's. Without it the index cannot be extrapolated to now.
    index_timestamp    Nullable(DateTime('UTC')),

    -- From UpdateAssetConfig. Only `liquidityFee` is read — it is the fee rate
    -- in the supply side's `unrealizedFees` term (§5.2). The other three config
    -- fields are addresses nothing values.
    liquidity_fee      Nullable(UInt16),

    -- From AddAsset. Immutable in practice, but stored through the same
    -- mechanism so a re-listing would be handled rather than ignored.
    underlying         Nullable(String),
    decimals           Nullable(UInt8),

    -- +1 live, -1 retraction.
    sign               Int8,

    -- **An index for "which ERC-20s has the Hub listed", which is otherwise a
    -- scan.** Enrichment needs that set, and asking for it directly is
    -- `SELECT DISTINCT underlying ... WHERE underlying IS NOT NULL` —
    -- `underlying` is not in the sorting key and `chain_id` prunes nothing on a
    -- single-chain deployment, so `DISTINCT` walks the whole column to build
    -- its set. Measured at 600,017 rows: `Parts: 4/4, Granules: 76/76`. Since
    -- `UpdateAsset` writes a NULL `underlying` every time it fires — 434 per
    -- 10,000 blocks, about 1.5 million rows a year — the cost of the question
    -- grows with history that has no bearing on the answer.
    --
    -- With this, the same query reads **17 rows and 935 bytes in 2 ms**,
    -- `Granules: 1/76`, and `system.query_log.projections` names it. The query
    -- does not change: a projection is chosen by the optimizer, so the caller
    -- keeps asking the obvious question and stops paying for it.
    --
    -- **`count()` is not for reading.** It is the minimal aggregate that makes
    -- this a grouped projection — eighteen rows rather than a re-sorted copy of
    -- every row — and it counts rows *as written*, before the collapse below
    -- has cancelled anything. Only the set of `underlying` values is safe to
    -- trust here, which is all enrichment wants: it is deliberately lax about
    -- retraction, because a listing rolled back by a reorg costs one wasted
    -- ERC-20 read and nothing else.
    PROJECTION listed_tokens
    (
        SELECT
            chain_id,
            underlying,
            count()
        GROUP BY chain_id, underlying
    )
)
ENGINE = VersionedCollapsingMergeTree(sign, version)
PARTITION BY chain_id
ORDER BY (chain_id, hub, asset_id, block_number, log_index)
-- **Required, and the table is refused without it.** ClickHouse rejects a
-- projection on a `VersionedCollapsingMergeTree` under the default `throw` —
-- "not supported ... with deduplicate_merge_projection_mode = throw" — because
-- a projection aggregates rows as written and cannot see the collapse `sign`
-- encodes. `rebuild` recomputes it from merged data instead of dropping it,
-- which is what keeps it usable; the caveat that comes with that is the one
-- spelled out on `count()` above.
SETTINGS deduplicate_merge_projection_mode = 'rebuild';
