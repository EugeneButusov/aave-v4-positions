-- `041_hub_refresh_premium`, writing to the event-grain table and emitting the
-- instant. The projection is otherwise the one that was reviewed.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_refresh_premium TO hub_asset_deltas AS
SELECT
    chain_id,
    address                                       AS hub,
    toUInt256(JSONExtractString(body, 'assetId')) AS asset_id,
    block_timestamp,
    toInt256(0)                                   AS liquidity,
    toInt256(0)                                   AS added_shares,
    toInt256(0)                                   AS drawn_shares,
    toInt256(0)                                   AS swept,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))
                                                  AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta'))
                                                  AS premium_offset_ray,
    toInt256(0)                                   AS deficit_ray,
    toInt64(sign)                                 AS events
FROM hub_events
WHERE event_name = 'RefreshPremium';
