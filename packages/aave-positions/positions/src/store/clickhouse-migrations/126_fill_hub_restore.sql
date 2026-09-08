-- The history `hub_restore` will not see, replayed from the log.
-- Inserting into `hub_asset_deltas` fires the rollup at `122`, so `hub_assets`
-- is rebuilt by the same statement rather than needing a backfill of its own.
-- Columns named, and they have to be. `INSERT … SELECT` maps by position,
-- `ALTER TABLE … ADD COLUMN` appends, and ClickHouse raises nothing when the
-- two disagree — it truncated a DateTime into a UInt8 flag when measured, and
-- wrote it. A materialized view maps by name and is safe either way; this is
-- the statement that is not.
INSERT INTO hub_asset_deltas
    (chain_id, hub, asset_id, block_timestamp, liquidity, added_shares,
     drawn_shares, swept, premium_shares, premium_offset_ray, deficit_ray, events)
SELECT
    chain_id,
    address                                                     AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))               AS asset_id,
    block_timestamp,
    sign * (toInt256(JSONExtractString(body, 'drawnAmount'))
          + toInt256(JSONExtractString(body, 'premiumAmount'))) AS liquidity,
    toInt256(0)                                                 AS added_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnShares'))    AS drawn_shares,
    toInt256(0)                                                 AS swept,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))
                                                                AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta'))
                                                                AS premium_offset_ray,
    toInt256(0)                                                 AS deficit_ray,
    toInt64(sign)                                               AS events
FROM hub_events
WHERE event_name = 'Restore';
