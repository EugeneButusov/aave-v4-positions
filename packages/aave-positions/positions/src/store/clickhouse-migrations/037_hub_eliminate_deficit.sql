-- EliminateDeficit: a spoke covers bad debt by burning its own supply shares.
--
-- **Not in the analysis's transition table at all** — read from
-- `Hub.eliminateDeficit:333-359`: `asset.addedShares -= shares` and
-- `asset.deficitRay -= deficitAmountRay`, falling together so the share price is
-- preserved and other suppliers take no haircut (§12.3).
--
-- `topic2` is the *calling* spoke and `topic3` the covered one. Neither is read
-- here, because both quantities are asset-global.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_eliminate_deficit TO hub_assets AS
SELECT
    chain_id,
    address                                                      AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))                AS asset_id,
    toInt256(0)                                                  AS liquidity,
    sign * -toInt256(JSONExtractString(body, 'shares'))          AS added_shares,
    toInt256(0)                                                  AS drawn_shares,
    toInt256(0)                                                  AS swept,
    toInt256(0)                                                  AS premium_shares,
    toInt256(0)                                                  AS premium_offset_ray,
    sign * -toInt256(JSONExtractString(body, 'deficitAmountRay')) AS deficit_ray,
    toInt64(sign)                                                AS events
FROM hub_events
WHERE event_name = 'EliminateDeficit';
