-- `038_hub_mint_fee_shares`, writing to the event-grain table and emitting the
-- instant. The projection is otherwise the one that was reviewed.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_mint_fee_shares TO hub_asset_deltas AS
SELECT
    chain_id,
    address                                                AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))          AS asset_id,
    block_timestamp,
    toInt256(0)                                            AS liquidity,
    sign * toInt256(JSONExtractString(body, 'shares'))     AS added_shares,
    toInt256(0)                                            AS drawn_shares,
    toInt256(0)                                            AS swept,
    toInt256(0)                                            AS premium_shares,
    toInt256(0)                                            AS premium_offset_ray,
    toInt256(0)                                            AS deficit_ray,
    toInt64(sign)                                          AS events
FROM hub_events
WHERE event_name = 'MintFeeShares';
