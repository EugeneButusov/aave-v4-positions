-- The latest-wins half of the Hub asset mirror.
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
    sign               Int8
)
ENGINE = VersionedCollapsingMergeTree(sign, version)
PARTITION BY chain_id
ORDER BY (chain_id, hub, asset_id, block_number, log_index)
