-- The history `position_liquidation_debt` will not see, replayed from the log.
-- A materialized view fires on insert and never looks back, so the rows written
-- before `076` existed have to come from `spoke_events` directly. Same
-- projection, so the two cannot disagree.
INSERT INTO user_positions
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                     AS user,
    address                                                                    AS spoke,
    toUInt256(JSONExtractString(body, 'debtReserveId'))                        AS reserve_id,
    block_timestamp,
    toInt256(0)                                                                AS supplied_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnSharesLiquidated'))         AS drawn_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))    AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta')) AS premium_offset_ray,
    toInt256(0)                                                                AS net_supplied_amount,
    sign * -toInt256(JSONExtractString(body, 'debtAmountRestored'))            AS net_borrowed_amount,
    toInt64(sign)                                                              AS events
FROM spoke_events
WHERE event_name = 'LiquidationCall';
