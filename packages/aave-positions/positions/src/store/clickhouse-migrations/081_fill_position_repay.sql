-- The history `position_repay` will not see, replayed from the log.
-- A materialized view fires on insert and never looks back, so the rows written
-- before `073` existed have to come from `spoke_events` directly. Same
-- projection, so the two cannot disagree.
-- Columns named, and they have to be. `INSERT … SELECT` maps by position,
-- `ALTER TABLE … ADD COLUMN` appends, and ClickHouse raises nothing when the
-- two disagree — it truncated a DateTime into a UInt8 flag when measured, and
-- wrote it. A materialized view maps by name and is safe either way; this is
-- the statement that is not.
INSERT INTO user_positions
    (chain_id, user, spoke, reserve_id, block_timestamp, supplied_shares,
     drawn_shares, premium_shares, premium_offset_ray, net_supplied_amount,
     net_borrowed_amount, events)
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                        AS user,
    address                                                                       AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))                               AS reserve_id,
    block_timestamp,
    toInt256(0)                                                                   AS supplied_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnShares'))                      AS drawn_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))       AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta'))    AS premium_offset_ray,
    toInt256(0)                                                                   AS net_supplied_amount,
    sign * -toInt256(JSONExtractString(body, 'totalAmountRepaid'))                AS net_borrowed_amount,
    toInt64(sign)                                                                 AS events
FROM spoke_events
WHERE event_name = 'Repay';
