-- The projection of `AddReserve`, and the view that resolves it.
--
-- Two statements because they are one change: a registry nothing can read is
-- not a registry. The same `--@statement` marker the runner splits on.

--@statement

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
WHERE event_name = 'AddReserve'

--@statement

-- Latest listing wins, over rows the collapse has already filtered — the same
-- shape as every other latest-wins read here, and ordered by chain order rather
-- than `version` for the same measured reason.
CREATE VIEW IF NOT EXISTS spoke_reserves_current AS
SELECT
    chain_id,
    spoke,
    reserve_id,
    argMax(asset_id, (block_number, log_index)) AS asset_id,
    argMax(hub, (block_number, log_index))      AS hub
FROM
(
    SELECT
        chain_id, spoke, reserve_id, block_number, log_index,
        any(asset_id) AS asset_id,
        any(hub)      AS hub
    FROM spoke_reserves
    GROUP BY chain_id, spoke, reserve_id, block_number, log_index, version
    HAVING sum(sign) > 0
)
GROUP BY chain_id, spoke, reserve_id
