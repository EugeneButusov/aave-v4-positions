-- AddReserve -> spoke_reserves. Copies version and sign verbatim, as 021 does.
--
-- Not joined into the position row at insert time. A materialized view sees only
-- the block being inserted, so a Supply arriving in the same insert as the
-- AddReserve that names its asset would find nothing to join against — and the
-- backfill's first chunk is exactly that case. Resolved at read time instead.
CREATE MATERIALIZED VIEW IF NOT EXISTS spoke_reserve_registry TO spoke_reserves AS
SELECT
    chain_id,
    address                                         AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId')) AS reserve_id,
    block_number,
    log_index,
    version,
    toUInt256(JSONExtractString(body, 'assetId'))   AS asset_id,
    lower(JSONExtractString(body, 'hub'))           AS hub,
    sign
FROM spoke_events
WHERE event_name = 'AddReserve'
