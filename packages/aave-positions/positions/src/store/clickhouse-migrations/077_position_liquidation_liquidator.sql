-- `019_position_liquidation_liquidator`, emitting the instant as well as the delta.
-- Identical otherwise; the projection is the one that was reviewed.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_liquidation_liquidator TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'liquidator'))                              AS user,
    address                                                                   AS spoke,
    toUInt256(JSONExtractString(body, 'collateralReserveId'))                 AS reserve_id,
    block_timestamp,
    sign * toInt256(JSONExtractString(body, 'collateralSharesToLiquidator'))  AS supplied_shares,
    toInt256(0)                                                               AS drawn_shares,
    toInt256(0)                                                               AS premium_shares,
    toInt256(0)                                                               AS premium_offset_ray,
    toInt256(0)                                                               AS net_supplied_amount,
    toInt256(0)                                                               AS net_borrowed_amount,
    toInt64(sign)                                                             AS events
FROM spoke_events
WHERE event_name = 'LiquidationCall' AND JSONExtractBool(body, 'receiveShares');
