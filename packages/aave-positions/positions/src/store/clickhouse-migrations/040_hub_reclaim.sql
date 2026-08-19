-- Reclaim: swept liquidity comes back. The exact inverse of Sweep.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_reclaim TO hub_assets AS
SELECT
    chain_id,
    address                                                AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))          AS asset_id,
    sign * toInt256(JSONExtractString(body, 'amount'))     AS liquidity,
    toInt256(0)                                            AS added_shares,
    toInt256(0)                                            AS drawn_shares,
    sign * -toInt256(JSONExtractString(body, 'amount'))    AS swept,
    toInt256(0)                                            AS premium_shares,
    toInt256(0)                                            AS premium_offset_ray,
    toInt256(0)                                            AS deficit_ray,
    toInt64(sign)                                          AS events
FROM hub_events
WHERE event_name = 'Reclaim';
