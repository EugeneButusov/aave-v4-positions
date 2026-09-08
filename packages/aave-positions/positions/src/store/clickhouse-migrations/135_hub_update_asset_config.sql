-- `043_hub_update_asset_config`, emitting the block's instant as well.
-- `MODIFY QUERY` rather than a recreate: the target only gained a column.
ALTER TABLE hub_update_asset_config MODIFY QUERY
SELECT
    chain_id,
    address                                                        AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))                  AS asset_id,
    block_number,
    log_index,
    version,
    block_timestamp,
    CAST(NULL, 'Nullable(UInt256)')                                AS drawn_index,
    CAST(NULL, 'Nullable(UInt256)')                                AS drawn_rate,
    CAST(NULL, 'Nullable(UInt256)')                                AS realized_fees,
    CAST(NULL, 'Nullable(DateTime(\'UTC\'))')                      AS index_timestamp,
    toUInt16(JSONExtractString(body, 'config', 'liquidityFee'))    AS liquidity_fee,
    CAST(NULL, 'Nullable(String)')                                 AS underlying,
    CAST(NULL, 'Nullable(UInt8)')                                  AS decimals,
    sign
FROM hub_events
WHERE event_name = 'UpdateAssetConfig';
