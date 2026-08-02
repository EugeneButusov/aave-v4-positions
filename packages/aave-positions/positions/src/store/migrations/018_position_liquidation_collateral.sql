-- LiquidationCall, leg 1 of 3: collateral seized from the borrower.
--
-- One log affects up to three positions, so it gets three projections rather
-- than one arrayJoin — each stays readable and each fails on its own.
--
-- The seized reserve is `collateralReserveId`, which is topic1. Self-liquidation
-- — USDC debt against USDC collateral — puts this leg and the next on the same
-- key, and SummingMergeTree simply adds them into one row carrying both deltas.
-- That is correct, and it is why no leg discriminator column exists; under a
-- collapsing engine it would have needed one.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_liquidation_collateral TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                   AS user,
    address                                                                  AS spoke,
    toUInt256(JSONExtractString(body, 'collateralReserveId'))                AS reserve_id,
    sign * -toInt256(JSONExtractString(body, 'collateralSharesLiquidated'))  AS supplied_shares,
    toInt256(0)                                                              AS drawn_shares,
    toInt256(0)                                                              AS premium_shares,
    toInt256(0)                                                              AS premium_offset_ray,
    sign * -toInt256(JSONExtractString(body, 'collateralAmountRemoved'))     AS net_supplied_amount,
    toInt256(0)                                                              AS net_borrowed_amount,
    toInt64(sign)                                                            AS events
FROM spoke_events
WHERE event_name = 'LiquidationCall'
