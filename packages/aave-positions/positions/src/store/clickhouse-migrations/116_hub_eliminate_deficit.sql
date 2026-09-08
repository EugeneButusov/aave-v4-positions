-- `037_hub_eliminate_deficit`, writing to the event-grain table and emitting the
-- instant. The projection is otherwise the one that was reviewed.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_eliminate_deficit TO hub_asset_deltas AS
SELECT
    chain_id,
    address                                                      AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))                AS asset_id,
    block_timestamp,
    toInt256(0)                                                  AS liquidity,
    sign * -toInt256(JSONExtractString(body, 'shares'))          AS added_shares,
    toInt256(0)                                                  AS drawn_shares,
    toInt256(0)                                                  AS swept,
    toInt256(0)                                                  AS premium_shares,
    toInt256(0)                                                  AS premium_offset_ray,
    sign * -toInt256(JSONExtractString(body, 'deficitAmountRay')) AS deficit_ray,
    toInt64(sign)                                                AS events
FROM hub_events
WHERE event_name = 'EliminateDeficit';
