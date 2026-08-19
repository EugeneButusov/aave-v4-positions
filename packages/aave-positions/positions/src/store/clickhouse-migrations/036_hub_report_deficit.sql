-- ReportDeficit: debt written off as bad. Shares leave without cash arriving,
-- and the loss is recorded RAY-scaled in `deficitRay`, where it stays inside
-- `aggregatedOwedRay` until eliminated (§12.3) — so suppliers still hold shares
-- backed by it.
--
-- The Hub's five-parameter form. The Spoke emits a *different* four-parameter
-- event of the same name against a different ledger (§4.4).
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_report_deficit TO hub_assets AS
SELECT
    chain_id,
    address                                                     AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))               AS asset_id,
    toInt256(0)                                                 AS liquidity,
    toInt256(0)                                                 AS added_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnShares'))    AS drawn_shares,
    toInt256(0)                                                 AS swept,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))
                                                                AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta'))
                                                                AS premium_offset_ray,
    sign * toInt256(JSONExtractString(body, 'deficitAmountRay')) AS deficit_ray,
    toInt64(sign)                                               AS events
FROM hub_events
WHERE event_name = 'ReportDeficit';
