-- `015_position_repay`, emitting the instant as well as the delta.
-- Identical otherwise; the projection is the one that was reviewed.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_repay TO user_positions AS
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
