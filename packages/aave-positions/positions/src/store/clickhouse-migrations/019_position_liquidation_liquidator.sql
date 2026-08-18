-- LiquidationCall, leg 3 of 3: collateral credited to the liquidator.
--
-- §4.1's trap 3, and the reason this view exists at all: when `receiveShares`
-- is true the collateral never leaves the Hub, ownership moves inside the Spoke,
-- and the liquidator's position grows with **no Supply event anywhere in the
-- trace**. Crediting supplied shares only on Supply silently under-counts every
-- liquidator, with nothing in the logs to indicate a problem.
--
-- It has never fired on mainnet — 0 of 90 liquidations — which is the argument
-- for folding it now rather than later: it cannot be tested against production
-- data when it first appears, and it is cheap while the transition table is
-- fresh. A synthetic spec is the only proof it will ever have.
--
-- No asset-amount leg: nothing was transferred, so `collateralAmountRemoved`
-- belongs to leg 1 alone. The gap between `collateralSharesLiquidated` and
-- `collateralSharesToLiquidator` is the protocol fee, which settles as a Hub
-- TransferShares to the treasury spoke and touches no user position.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_liquidation_liquidator TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'liquidator'))                              AS user,
    address                                                                   AS spoke,
    toUInt256(JSONExtractString(body, 'collateralReserveId'))                 AS reserve_id,
    sign * toInt256(JSONExtractString(body, 'collateralSharesToLiquidator'))  AS supplied_shares,
    toInt256(0)                                                               AS drawn_shares,
    toInt256(0)                                                               AS premium_shares,
    toInt256(0)                                                               AS premium_offset_ray,
    toInt256(0)                                                               AS net_supplied_amount,
    toInt256(0)                                                               AS net_borrowed_amount,
    toInt64(sign)                                                             AS events
FROM spoke_events
WHERE event_name = 'LiquidationCall' AND JSONExtractBool(body, 'receiveShares');
