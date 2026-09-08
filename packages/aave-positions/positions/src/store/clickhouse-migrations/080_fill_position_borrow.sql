-- The history `position_borrow` will not see, replayed from the log.
-- A materialized view fires on insert and never looks back, so the rows written
-- before `072` existed have to come from `spoke_events` directly. Same
-- projection, so the two cannot disagree.
INSERT INTO user_positions
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                    AS user,
    address                                                   AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))           AS reserve_id,
    block_timestamp,
    toInt256(0)                                               AS supplied_shares,
    sign * toInt256(JSONExtractString(body, 'drawnShares'))   AS drawn_shares,
    toInt256(0)                                               AS premium_shares,
    toInt256(0)                                               AS premium_offset_ray,
    toInt256(0)                                               AS net_supplied_amount,
    sign * toInt256(JSONExtractString(body, 'drawnAmount'))   AS net_borrowed_amount,
    toInt64(sign)                                             AS events
FROM spoke_events
WHERE event_name = 'Borrow';
