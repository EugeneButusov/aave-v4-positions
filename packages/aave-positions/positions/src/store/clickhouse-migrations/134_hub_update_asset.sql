-- `042_hub_update_asset`, emitting the block's instant as well.
-- `MODIFY QUERY` rather than a recreate: the target only gained a column.
ALTER TABLE hub_update_asset MODIFY QUERY
SELECT
    chain_id,
    address                                                       AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))                 AS asset_id,
    block_number,
    log_index,
    version,
    block_timestamp,
    toUInt256(JSONExtractString(body, 'drawnIndex'))              AS drawn_index,
    toUInt256(JSONExtractString(body, 'drawnRate'))               AS drawn_rate,
    toUInt256(JSONExtractString(body, 'accruedFees'))             AS realized_fees,
    block_timestamp                                               AS index_timestamp,
    CAST(NULL, 'Nullable(UInt16)')                                AS liquidity_fee,
    CAST(NULL, 'Nullable(String)')                                AS underlying,
    CAST(NULL, 'Nullable(UInt8)')                                 AS decimals,
    sign
FROM hub_events
WHERE event_name = 'UpdateAsset';
