-- Every reserve ever listed, replayed from the log, with `093`'s projection
-- exactly — so the history and the rows written from here on cannot disagree.
-- Columns named, and they have to be. `INSERT … SELECT` maps by position,
-- `ALTER TABLE … ADD COLUMN` appends, and ClickHouse raises nothing when the
-- two disagree — it truncated a DateTime into a UInt8 flag when measured, and
-- wrote it. A materialized view maps by name and is safe either way; this is
-- the statement that is not.
INSERT INTO spoke_reserves
    (chain_id, spoke, reserve_id, block_number, log_index, version,
     block_timestamp, asset_id, hub, sign)
SELECT
    chain_id,
    address                                        AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId')) AS reserve_id,
    block_number,
    log_index,
    version,
    block_timestamp,
    toUInt256(JSONExtractString(body, 'assetId'))  AS asset_id,
    lower(JSONExtractString(body, 'hub'))          AS hub,
    sign
FROM spoke_events
WHERE event_name = 'AddReserve';
