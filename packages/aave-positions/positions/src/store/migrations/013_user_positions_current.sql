-- The position, both halves, as one row. Every read goes through this.
--
-- **UNION ALL rather than a JOIN**, which is a performance decision and not a
-- stylistic one. ClickHouse has no index-seek join: every hash variant reads the
-- entire right side into memory to build a hash table, and full_sorting_merge
-- sorts both sides rather than exploiting that they are already ordered by the
-- join key. A LEFT JOIN here would therefore scan, aggregate and hash the whole
-- flag table on every query unless the planner happened to push the predicate
-- into it — and pushdown through a join is exactly the fragile case. Pushdown
-- into UNION ALL branches is not: each branch prunes to its own key range.
--
-- Concatenating also costs nothing extra, because SummingMergeTree merges parts
-- in the background and a key can have several rows until it does. The GROUP BY
-- was already required; the flag rows just join the aggregation that was
-- happening anyway.
--
-- LEFT JOIN semantics fall out of it: a position with no flag event still
-- appears, via ifNull, and a flag with no position sums to zero shares and is
-- dropped by the store's filter.
CREATE VIEW IF NOT EXISTS user_positions_current AS
SELECT
    chain_id,
    user,
    spoke,
    reserve_id,
    sum(supplied_shares)     AS supplied_shares,
    sum(drawn_shares)        AS drawn_shares,
    sum(premium_shares)      AS premium_shares,
    sum(premium_offset_ray)  AS premium_offset_ray,
    sum(net_supplied_amount) AS net_supplied_amount,
    sum(net_borrowed_amount) AS net_borrowed_amount,
    sum(events)              AS events,
    -- Ordered by (block_number, log_index) — chain order. Ordering by `version`
    -- instead reads the stale flag whenever a range is re-dispatched out of
    -- order, which the loop does whenever a later processor asks to retry.
    ifNull(argMaxIf(flag, (block_number, log_index), flag IS NOT NULL), 0)
                             AS using_as_collateral
FROM
(
    SELECT
        chain_id, user, spoke, reserve_id,
        supplied_shares, drawn_shares, premium_shares, premium_offset_ray,
        net_supplied_amount, net_borrowed_amount, events,
        CAST(NULL, 'Nullable(UInt8)') AS flag,
        toUInt64(0)                   AS block_number,
        toUInt32(0)                   AS log_index
    FROM user_positions

    UNION ALL

    SELECT
        chain_id, user, spoke, reserve_id,
        toInt256(0), toInt256(0), toInt256(0), toInt256(0),
        toInt256(0), toInt256(0), toInt64(0),
        using_as_collateral, block_number, log_index
    FROM
    (
        -- The collapse, and it is load-bearing rather than tidy. FINAL leaks an
        -- unpaired retraction where this does not — measured, 1 row against 0 —
        -- and grouping without `version` would let a reorg's superseded row and
        -- its replacement merge, so any() could return the stale content.
        -- Without it a retracted flag stays the argMax forever.
        SELECT
            chain_id, user, spoke, reserve_id, block_number, log_index,
            any(using_as_collateral) AS using_as_collateral
        FROM user_position_flags
        GROUP BY chain_id, user, spoke, reserve_id, block_number, log_index, version
        HAVING sum(sign) > 0
    )
)
GROUP BY chain_id, user, spoke, reserve_id
