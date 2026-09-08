-- The history `position_supply` will not see, replayed from the log.
-- A materialized view fires on insert and never looks back, so the rows written
-- before `070` existed have to come from `spoke_events` directly. Same
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
    lower(JSONExtractString(body, 'user'))                      AS user,
    address                                                     AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))             AS reserve_id,
    block_timestamp,
    sign * toInt256(JSONExtractString(body, 'suppliedShares'))  AS supplied_shares,
    toInt256(0)                                                 AS drawn_shares,
    toInt256(0)                                                 AS premium_shares,
    toInt256(0)                                                 AS premium_offset_ray,
    sign * toInt256(JSONExtractString(body, 'suppliedAmount'))  AS net_supplied_amount,
    toInt256(0)                                                 AS net_borrowed_amount,
    toInt64(sign)                                               AS events
FROM spoke_events
WHERE event_name = 'Supply';
