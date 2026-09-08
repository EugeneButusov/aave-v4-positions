-- Every flag ever set, replayed from the log.
--
-- The projection is `087`'s exactly, which is what keeps the history and the
-- rows written from here on from disagreeing. Retracted flags come back as
-- their `sign = -1` twins too, so the collapse the read view does is unchanged.
INSERT INTO user_position_flags
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
