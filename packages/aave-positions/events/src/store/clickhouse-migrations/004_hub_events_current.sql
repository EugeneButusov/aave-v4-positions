-- Every read goes through this, never the raw table. The Hub twin of
-- `spoke_events_current`, for the same measured reasons.
--
-- FINAL does *not* filter an unpaired retraction row, so a consumer reading the
-- table directly would see retractions as though they were events.
--
-- Grouping includes version because that is half the pairing key. Grouping by
-- the sorting key alone would let a reorg's superseded row and its replacement
-- collapse into one group, and any() could then return the stale content.
--
-- any() is exact rather than arbitrary: a retraction copies every column, so a
-- cancelled pair is two identical rows netting to sum(sign) = 0, and a live
-- group holds exactly one row.
CREATE VIEW IF NOT EXISTS hub_events_current AS
SELECT
    chain_id,
    block_number,
    log_index,
    version,
    any(address)         AS address,
    any(block_hash)      AS block_hash,
    any(block_timestamp) AS block_timestamp,
    any(tx_hash)         AS tx_hash,
    any(tx_index)        AS tx_index,
    any(event_name)      AS event_name,
    any(topic1)          AS topic1,
    any(topic2)          AS topic2,
    any(topic3)          AS topic3,
    any(body)            AS body,
    any(data)            AS data
FROM hub_events
GROUP BY chain_id, block_number, log_index, version
HAVING sum(sign) > 0;
