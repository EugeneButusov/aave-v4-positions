-- Every reserve ever listed, replayed from the log, with `093`'s projection
-- exactly — so the history and the rows written from here on cannot disagree.
INSERT INTO spoke_reserves
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
