-- RefreshPremium: the risk premium moves with no cash and no shares changing
-- hands. **Not in the analysis's transition table** — read from
-- `Hub.refreshPremium:362-374`, which calls the same `_applyPremiumDelta` as
-- Restore and ReportDeficit.
--
-- `_validateApplyPremiumDelta` adds both deltas verbatim —
-- `premiumShares.add(sharesDelta)` and `premiumOffsetRay + offsetRayDelta` —
-- which is precisely what makes the pair additive rather than latest-wins.
-- `restoredPremiumRay` is required to be zero here and changes no stored field.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_refresh_premium TO hub_assets AS
SELECT
    chain_id,
    address                                       AS hub,
    toUInt256(JSONExtractString(body, 'assetId')) AS asset_id,
    toInt256(0)                                   AS liquidity,
    toInt256(0)                                   AS added_shares,
    toInt256(0)                                   AS drawn_shares,
    toInt256(0)                                   AS swept,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))
                                                  AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta'))
                                                  AS premium_offset_ray,
    toInt256(0)                                   AS deficit_ray,
    toInt64(sign)                                 AS events
FROM hub_events
WHERE event_name = 'RefreshPremium';
