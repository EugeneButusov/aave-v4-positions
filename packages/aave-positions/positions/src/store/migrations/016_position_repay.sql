-- Repay: debt shares out, plus the premium components. See 013.
--
-- `premiumDelta` is an int256 triple and is summed exactly as emitted — it is a
-- delta, so it adds. Nothing on-chain confirms that reading: the premium has
-- never been non-zero on mainnet (§5.4), so the sign convention is pinned by a
-- synthetic spec and the reconciliation cannot check it.
--
-- The six-field shape is Repay's alone: it inserts `totalAmountRepaid` where
-- the other three hot events carry a plain `amount`.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_repay TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                        AS user,
    address                                                                       AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))                               AS reserve_id,
    toInt256(0)                                                                   AS supplied_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnShares'))                      AS drawn_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))       AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta'))    AS premium_offset_ray,
    toInt256(0)                                                                   AS net_supplied_amount,
    sign * -toInt256(JSONExtractString(body, 'totalAmountRepaid'))                AS net_borrowed_amount,
    toInt64(sign)                                                                 AS events
FROM spoke_events
WHERE event_name = 'Repay'
