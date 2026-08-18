-- Restore: debt is repaid. Debt shares down, and **liquidity rises by
-- `drawnAmount + premiumAmount`** — the premium is cash that arrives too, which
-- a fold reading only `drawnAmount` would lose.
--
-- Also carries a premium delta, applied at asset level by
-- `_applyPremiumDelta(asset, spoke, …)`.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_restore TO hub_assets AS
SELECT
    chain_id,
    address                                                     AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))               AS asset_id,
    sign * (toInt256(JSONExtractString(body, 'drawnAmount'))
          + toInt256(JSONExtractString(body, 'premiumAmount'))) AS liquidity,
    toInt256(0)                                                 AS added_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnShares'))    AS drawn_shares,
    toInt256(0)                                                 AS swept,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))
                                                                AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta'))
                                                                AS premium_offset_ray,
    toInt256(0)                                                 AS deficit_ray,
    toInt64(sign)                                               AS events
FROM hub_events
WHERE event_name = 'Restore';
