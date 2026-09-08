-- `020_position_collateral_flag`, emitting the instant as well as the flag.
--
-- `MODIFY QUERY` rather than a drop and recreate, which the position views
-- needed: their target table changed shape underneath them, this one only
-- gained a column.
ALTER TABLE position_collateral_flag MODIFY QUERY
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
