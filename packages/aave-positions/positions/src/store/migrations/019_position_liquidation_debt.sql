-- LiquidationCall, leg 2 of 3: debt restored on the borrower.
--
-- A different reserve from leg 1 — `debtReserveId`, which is topic2 — so this is
-- a different position for the same user, and the two legs of one liquidation
-- land on two rows. See 018 for the self-liquidation case where they do not.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_liquidation_debt TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                     AS user,
    address                                                                    AS spoke,
    toUInt256(JSONExtractString(body, 'debtReserveId'))                        AS reserve_id,
    toInt256(0)                                                                AS supplied_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnSharesLiquidated'))         AS drawn_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))    AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta')) AS premium_offset_ray,
    toInt256(0)                                                                AS net_supplied_amount,
    sign * -toInt256(JSONExtractString(body, 'debtAmountRestored'))            AS net_borrowed_amount,
    toInt64(sign)                                                              AS events
FROM spoke_events
WHERE event_name = 'LiquidationCall'
