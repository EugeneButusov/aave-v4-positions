-- One Hub asset, both halves, as one row. Every read goes through this.
--
-- The same `UNION ALL`-not-`JOIN` shape as `user_positions_current`, for the
-- same measured reason: ClickHouse has no index-seek join, so a LEFT JOIN would
-- scan, aggregate and hash the whole state table on every query unless the
-- planner pushed the predicate through — and pushdown through a join is the
-- fragile case where pushdown into union branches is not.
--
-- **Three latest-wins groups, resolved one column at a time.** `UpdateAsset`,
-- `UpdateAssetConfig` and `AddAsset` write disjoint columns of one row and fire
-- at wildly different rates — 29,482 against 34 against 17 over all history — so
-- the newest row for an asset is almost always an `UpdateAsset` whose
-- `underlying` is NULL. Resolving the row as a whole would blank the listing
-- fields every twenty seconds; each column has to find its own newest value.
--
-- The `If` is explicitness rather than necessity, and the mutation test says so:
-- replacing `argMaxIf(underlying, …, underlying IS NOT NULL)` with a plain
-- `argMax` changes no result, because **`argMax` already ignores rows whose
-- argument is NULL** — measured, `argMax(v, ord)` over
-- `(100,'usdc'), (200,NULL), (300,NULL)` returns `'usdc'`. Spelling the
-- condition out keeps the intent at the call site and keeps the view correct if
-- one of these columns ever stops being nullable.
CREATE VIEW IF NOT EXISTS hub_assets_current AS
SELECT
    chain_id,
    hub,
    asset_id,
    sum(liquidity)          AS liquidity,
    sum(added_shares)       AS added_shares,
    sum(drawn_shares)       AS drawn_shares,
    sum(swept)              AS swept,
    sum(premium_shares)     AS premium_shares,
    sum(premium_offset_ray) AS premium_offset_ray,
    sum(deficit_ray)        AS deficit_ray,
    sum(events)             AS events,
    -- Ordered by (block_number, log_index) — chain order. Ordering by `version`
    -- instead reads a stale checkpoint whenever a range is re-dispatched out of
    -- order, which the loop does whenever a later processor asks to retry.
    --
    -- This ordering is also what makes `realized_fees` correct across a
    -- MintFeeShares: the mint zeroes it and the `UpdateAsset` carrying the zero
    -- is emitted after, at a higher log_index, in the same transaction.
    argMaxIf(drawn_index, (block_number, log_index), drawn_index IS NOT NULL)
                            AS drawn_index,
    argMaxIf(drawn_rate, (block_number, log_index), drawn_rate IS NOT NULL)
                            AS drawn_rate,
    argMaxIf(realized_fees, (block_number, log_index), realized_fees IS NOT NULL)
                            AS realized_fees,
    argMaxIf(index_timestamp, (block_number, log_index), index_timestamp IS NOT NULL)
                            AS index_timestamp,
    argMaxIf(liquidity_fee, (block_number, log_index), liquidity_fee IS NOT NULL)
                            AS liquidity_fee,
    argMaxIf(underlying, (block_number, log_index), underlying IS NOT NULL)
                            AS underlying,
    argMaxIf(decimals, (block_number, log_index), decimals IS NOT NULL)
                            AS decimals
FROM
(
    SELECT
        chain_id, hub, asset_id,
        liquidity, added_shares, drawn_shares, swept,
        premium_shares, premium_offset_ray, deficit_ray, events,
        CAST(NULL, 'Nullable(UInt256)')           AS drawn_index,
        CAST(NULL, 'Nullable(UInt256)')           AS drawn_rate,
        CAST(NULL, 'Nullable(UInt256)')           AS realized_fees,
        CAST(NULL, 'Nullable(DateTime(\'UTC\'))') AS index_timestamp,
        CAST(NULL, 'Nullable(UInt16)')            AS liquidity_fee,
        CAST(NULL, 'Nullable(String)')            AS underlying,
        CAST(NULL, 'Nullable(UInt8)')             AS decimals,
        toUInt64(0)                               AS block_number,
        toUInt32(0)                               AS log_index
    FROM hub_assets

    UNION ALL

    SELECT
        chain_id, hub, asset_id,
        toInt256(0), toInt256(0), toInt256(0), toInt256(0),
        toInt256(0), toInt256(0), toInt256(0), toInt64(0),
        drawn_index, drawn_rate, realized_fees, index_timestamp,
        liquidity_fee, underlying, decimals,
        block_number, log_index
    FROM
    (
        -- The collapse, load-bearing rather than tidy. FINAL leaks an unpaired
        -- retraction where this does not — measured — and grouping without
        -- `version` would let a reorg's superseded row and its replacement
        -- merge, so any() could return stale content. Without it a retracted
        -- checkpoint stays the argMax forever.
        SELECT
            chain_id, hub, asset_id, block_number, log_index,
            any(drawn_index)     AS drawn_index,
            any(drawn_rate)      AS drawn_rate,
            any(realized_fees)   AS realized_fees,
            any(index_timestamp) AS index_timestamp,
            any(liquidity_fee)   AS liquidity_fee,
            any(underlying)      AS underlying,
            any(decimals)        AS decimals
        FROM hub_asset_state
        GROUP BY chain_id, hub, asset_id, block_number, log_index, version
        HAVING sum(sign) > 0
    )
)
GROUP BY chain_id, hub, asset_id;
