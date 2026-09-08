-- The history `hub_add_asset` will not see, replayed from the log.
--
-- Columns named, and they have to be. `INSERT … SELECT` maps by position,
-- `ALTER TABLE … ADD COLUMN` appends, and ClickHouse raises nothing when the two
-- disagree — it truncated a DateTime into a UInt8 when measured, and wrote it.
INSERT INTO hub_asset_state
    (chain_id, hub, asset_id, block_number, log_index, version, block_timestamp,
     drawn_index, drawn_rate, realized_fees, index_timestamp, liquidity_fee,
     underlying, decimals, sign)
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
