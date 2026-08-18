-- Draw: a Spoke borrows against Hub liquidity. Debt shares up, liquidity down.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_draw TO hub_assets AS
SELECT
    chain_id,
    address                                                     AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))               AS asset_id,
    sign * -toInt256(JSONExtractString(body, 'drawnAmount'))    AS liquidity,
    toInt256(0)                                                 AS added_shares,
    sign * toInt256(JSONExtractString(body, 'drawnShares'))     AS drawn_shares,
    toInt256(0)                                                 AS swept,
    toInt256(0)                                                 AS premium_shares,
    toInt256(0)                                                 AS premium_offset_ray,
    toInt256(0)                                                 AS deficit_ray,
    toInt64(sign)                                               AS events
FROM hub_events
WHERE event_name = 'Draw';
