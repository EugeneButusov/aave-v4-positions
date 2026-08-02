-- Borrow: debt shares in, assets out to the borrower. See 013.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_borrow TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                    AS user,
    address                                                   AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))           AS reserve_id,
    toInt256(0)                                               AS supplied_shares,
    sign * toInt256(JSONExtractString(body, 'drawnShares'))   AS drawn_shares,
    toInt256(0)                                               AS premium_shares,
    toInt256(0)                                               AS premium_offset_ray,
    toInt256(0)                                               AS net_supplied_amount,
    sign * toInt256(JSONExtractString(body, 'drawnAmount'))   AS net_borrowed_amount,
    toInt64(sign)                                             AS events
FROM spoke_events
WHERE event_name = 'Borrow'
