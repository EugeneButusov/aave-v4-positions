-- The projection of `AddReserve`. `052_spoke_reserves_current` reads it back;
-- the two are one change, applied as two files.

-- All three parameters are indexed, so the whole event is in the topics — but
-- it is read out of `body` like every other projection, because the decoder has
-- already turned the topic words into named fields and reading topic2 directly
-- is how §4.5's collision bites.
CREATE MATERIALIZED VIEW IF NOT EXISTS spoke_reserve_registry TO spoke_reserves AS
SELECT
    chain_id,
    address                                        AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId')) AS reserve_id,
    block_number,
    log_index,
    version,
    toUInt256(JSONExtractString(body, 'assetId'))  AS asset_id,
    lower(JSONExtractString(body, 'hub'))          AS hub,
    sign
FROM spoke_events
WHERE event_name = 'AddReserve';
