-- SetUsingAsCollateral -> user_position_flags.
--
-- The one projection that copies `version` and `sign` verbatim instead of
-- folding the sign into a value: this target collapses rather than sums, so the
-- retraction has to arrive as the pair's other half. See 011 for why the flag
-- cannot live in user_positions at all.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_collateral_flag TO user_position_flags AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))          AS user,
    address                                         AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId')) AS reserve_id,
    block_number,
    log_index,
    version,
    JSONExtractBool(body, 'usingAsCollateral')      AS using_as_collateral,
    sign
FROM spoke_events
WHERE event_name = 'SetUsingAsCollateral'
