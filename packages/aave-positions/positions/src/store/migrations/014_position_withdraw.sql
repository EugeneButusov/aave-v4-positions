-- Withdraw: shares out, assets out. See 013 for the three invariants.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_withdraw TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                       AS user,
    address                                                      AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))              AS reserve_id,
    sign * -toInt256(JSONExtractString(body, 'withdrawnShares')) AS supplied_shares,
    toInt256(0)                                                  AS drawn_shares,
    toInt256(0)                                                  AS premium_shares,
    toInt256(0)                                                  AS premium_offset_ray,
    sign * -toInt256(JSONExtractString(body, 'withdrawnAmount')) AS net_supplied_amount,
    toInt256(0)                                                  AS net_borrowed_amount,
    toInt64(sign)                                                AS events
FROM spoke_events
WHERE event_name = 'Withdraw'
