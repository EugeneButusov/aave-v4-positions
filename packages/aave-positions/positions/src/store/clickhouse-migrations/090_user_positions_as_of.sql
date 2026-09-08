-- One wallet's positions as they stood at a named instant.
--
-- **Replaces `user_positions_current`, and the name is the point.** A view
-- called "current" reads as the obvious right-hand side of any query, and the
-- page took it while asking to be valued at an instant the caller named — so
-- shares from now met an index from then. Naming the instant is not a
-- convenience here; it is the parameter that makes the answer well defined.
--
-- Both halves are cut, and they have to be. The additive half is a sum over
-- deltas, so the cut is membership: a supply after the instant simply is not in
-- it. The flag half is latest-wins, so the cut moves which row wins — a
-- collateral flag set after the instant must not be the one that argMax finds,
-- or a position reads as collateral before it ever was.
--
-- `sum(sign) > 0` still does the collapse, and still groups on `version`: a
-- reorg's superseded row and its replacement must not merge, or `any()` can
-- return stale content. The cut sits outside that, on the instant, so a
-- retraction and the row it retracts fall together whichever side they land on.
CREATE VIEW IF NOT EXISTS user_positions_as_of AS
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
    WHERE block_timestamp <= {cut:DateTime}
    UNION ALL
    SELECT
        chain_id, user, spoke, reserve_id,
        toInt256(0), toInt256(0), toInt256(0), toInt256(0),
        toInt256(0), toInt256(0), toInt64(0),
        using_as_collateral, block_number, log_index
    FROM
    (
        SELECT
            chain_id, user, spoke, reserve_id, block_number, log_index,
            any(using_as_collateral) AS using_as_collateral
        FROM user_position_flags
        WHERE block_timestamp <= {cut:DateTime}
        GROUP BY chain_id, user, spoke, reserve_id, block_number, log_index, version
        HAVING sum(sign) > 0
    )
)
GROUP BY chain_id, user, spoke, reserve_id;
