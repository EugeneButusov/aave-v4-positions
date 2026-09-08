-- `044_hub_add_asset`, emitting the block's instant as well.
-- `MODIFY QUERY` rather than a recreate: the target only gained a column.
ALTER TABLE hub_add_asset MODIFY QUERY
SELECT
    chain_id,
    address                                            AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))      AS asset_id,
    block_number,
    log_index,
    version,
    block_timestamp,
    CAST(NULL, 'Nullable(UInt256)')                    AS drawn_index,
    CAST(NULL, 'Nullable(UInt256)')                    AS drawn_rate,
    CAST(NULL, 'Nullable(UInt256)')                    AS realized_fees,
    CAST(NULL, 'Nullable(DateTime(\'UTC\'))')          AS index_timestamp,
    CAST(NULL, 'Nullable(UInt16)')                     AS liquidity_fee,
    lower(JSONExtractString(body, 'underlying'))       AS underlying,
    toUInt8(JSONExtractString(body, 'decimals'))       AS decimals,
    sign
FROM hub_events
WHERE event_name = 'AddAsset';
