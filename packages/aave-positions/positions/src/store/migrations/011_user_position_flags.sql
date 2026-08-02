-- The latest-wins half of a position, at event grain because it has to be.
--
-- `using_as_collateral` cannot be pre-aggregated under retraction, and the
-- alternatives were measured rather than argued away:
--
--   * AggregatingMergeTree(argMaxState) — argMaxState has no operation that
--     removes a contribution, so a sign = -1 row reinforces the state.
--   * ReplacingMergeTree at position grain — retracting means a tombstone at
--     that key, and Replacing deletes the *key*, not the generation, so the
--     position loses its flag instead of falling back to the previous value.
--   * argMaxIf(flag, (block_number, log_index), sign = 1) over raw rows —
--     measured 0 where the answer is 1. A retraction is a *pair* and the +1
--     twin still carries sign = 1: liveness is a property of the
--     (key, version) group, not of a row.
--
-- So the rows stay one-per-event and collapse exactly as the ledger does, and
-- the read view argMaxes over what survives. Roughly 3,198 rows over full
-- mainnet history.
CREATE TABLE IF NOT EXISTS user_position_flags
(
    chain_id            UInt32,
    user                String,
    spoke               String,
    reserve_id          UInt256,
    -- The ordering that decides "latest". Emphatically not `version`, which
    -- orders dispatches rather than chain events — measured: re-dispatching an
    -- earlier block after a later one gives it the higher version, and an
    -- argMax by version then returns the stale flag.
    block_number        UInt64,
    log_index           UInt32,
    -- Copied from the ledger row, because it is half the pairing key.
    version             UInt64,
    using_as_collateral UInt8,
    -- Copied from the ledger row. This is what makes the retraction pair.
    sign                Int8
)
ENGINE = VersionedCollapsingMergeTree(sign, version)
PARTITION BY chain_id
ORDER BY (chain_id, user, spoke, reserve_id, block_number, log_index)
