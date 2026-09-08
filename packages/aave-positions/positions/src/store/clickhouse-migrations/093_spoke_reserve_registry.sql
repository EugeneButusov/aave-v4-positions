-- `051_spoke_reserve_registry`, emitting the instant as well as the listing.
ALTER TABLE spoke_reserve_registry MODIFY QUERY
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
