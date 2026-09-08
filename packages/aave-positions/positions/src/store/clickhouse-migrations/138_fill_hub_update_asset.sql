-- The history `hub_update_asset` will not see, replayed from the log.
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
