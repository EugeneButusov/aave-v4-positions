-- Every flag ever set, replayed from the log.
--
-- The projection is `087`'s exactly, which is what keeps the history and the
-- rows written from here on from disagreeing. Retracted flags come back as
-- their `sign = -1` twins too, so the collapse the read view does is unchanged.
-- Columns named, and they have to be. `INSERT … SELECT` maps by position,
-- `ALTER TABLE … ADD COLUMN` appends, and ClickHouse raises nothing when the
-- two disagree — it truncated a DateTime into a UInt8 flag when measured, and
-- wrote it. A materialized view maps by name and is safe either way; this is
-- the statement that is not.
INSERT INTO user_position_flags
    (chain_id, user, spoke, reserve_id, block_number, log_index, version,
     block_timestamp, using_as_collateral, sign)
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))          AS user,
    address                                         AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId')) AS reserve_id,
    block_number,
    log_index,
    version,
    block_timestamp,
    JSONExtractBool(body, 'usingAsCollateral')      AS using_as_collateral,
    sign
FROM spoke_events
WHERE event_name = 'SetUsingAsCollateral';
