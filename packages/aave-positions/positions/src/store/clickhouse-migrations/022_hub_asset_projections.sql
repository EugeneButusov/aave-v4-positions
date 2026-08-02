-- Every projection of the Hub ledger, in one change.
--
-- Thirteen views: ten fold additive asset state into `hub_assets`, three record
-- a latest-wins value into `hub_asset_state`. One file because they are one
-- thing — the mirror — and §5.5 calls it the highest-risk fold in the design:
-- one mishandled transition silently corrupts every supply valuation for that
-- asset, with no error and nothing to flag it.
--
-- **The transitions below are transcribed from Hub.sol at commit 2524fe4**, not
-- from the analysis's summary table. Three of them the analysis does not cover
-- at all, and a fourth it states in a way that would mislead — see the comments
-- on EliminateDeficit, RefreshPremium, MintFeeShares and Add.
--
-- The three invariants from `012_position_projections.sql` hold in every view
-- here and are not repeated: read the table and never the view, multiply every
-- value by the source `sign`, and extract through JSONExtractString + toInt256
-- rather than JSONExtractInt, which returns 0 above Int64 max.
--
-- `topic1` is the asset id on all thirteen, but it is read out of `body`
-- alongside everything else so one expression style covers the file.

--@statement

-- Add: supply arrives. `addedShares += shares`, `liquidity += amount`.
--
-- Note `liquidity` is *assigned* on chain, not incremented —
-- `asset.liquidity = liquidity.toUint120()` where the local is
-- `asset.liquidity + amount`. Additive after all, but the `balanceOf` read
-- beside it is a solvency `require` rather than the source of the value, which
-- is worth knowing before trusting a fold of a field the contract assigns.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_add TO hub_assets AS
SELECT
    chain_id,
    address                                                AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))          AS asset_id,
    sign * toInt256(JSONExtractString(body, 'amount'))     AS liquidity,
    sign * toInt256(JSONExtractString(body, 'shares'))     AS added_shares,
    toInt256(0)                                            AS drawn_shares,
    toInt256(0)                                            AS swept,
    toInt256(0)                                            AS premium_shares,
    toInt256(0)                                            AS premium_offset_ray,
    toInt256(0)                                            AS deficit_ray,
    toInt64(sign)                                          AS events
FROM hub_events
WHERE event_name = 'Add'

--@statement

-- Remove: supply leaves. The exact inverse of Add.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_remove TO hub_assets AS
SELECT
    chain_id,
    address                                                AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))          AS asset_id,
    sign * -toInt256(JSONExtractString(body, 'amount'))    AS liquidity,
    sign * -toInt256(JSONExtractString(body, 'shares'))    AS added_shares,
    toInt256(0)                                            AS drawn_shares,
    toInt256(0)                                            AS swept,
    toInt256(0)                                            AS premium_shares,
    toInt256(0)                                            AS premium_offset_ray,
    toInt256(0)                                            AS deficit_ray,
    toInt64(sign)                                          AS events
FROM hub_events
WHERE event_name = 'Remove'

--@statement

-- Draw: a Spoke borrows against Hub liquidity. Debt shares up, liquidity down.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_draw TO hub_assets AS
SELECT
    chain_id,
    address                                                     AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))               AS asset_id,
    sign * -toInt256(JSONExtractString(body, 'drawnAmount'))    AS liquidity,
    toInt256(0)                                                 AS added_shares,
    sign * toInt256(JSONExtractString(body, 'drawnShares'))     AS drawn_shares,
    toInt256(0)                                                 AS swept,
    toInt256(0)                                                 AS premium_shares,
    toInt256(0)                                                 AS premium_offset_ray,
    toInt256(0)                                                 AS deficit_ray,
    toInt64(sign)                                               AS events
FROM hub_events
WHERE event_name = 'Draw'

--@statement

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
WHERE event_name = 'Restore'

--@statement

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
WHERE event_name = 'ReportDeficit'

--@statement

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
WHERE event_name = 'EliminateDeficit'

--@statement

-- MintFeeShares: accrued protocol fees are minted as supply shares to the fee
-- receiver. `asset.addedShares += shares`.
--
-- It also sets `asset.realizedFees = 0`, which is **not folded here** and must
-- not be: `realizedFees` is latest-wins in `hub_asset_state`, and
-- `_mintFeeShares` is always followed by `updateDrawnRate` in the same
-- transaction — so an `UpdateAsset` carrying the zeroed value lands at a higher
-- `log_index` and wins the argMax. Verified in Hub.sol: all 14 functions that
-- call `accrue()` also call `updateDrawnRate()`.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_mint_fee_shares TO hub_assets AS
SELECT
    chain_id,
    address                                                AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))          AS asset_id,
    toInt256(0)                                            AS liquidity,
    sign * toInt256(JSONExtractString(body, 'shares'))     AS added_shares,
    toInt256(0)                                            AS drawn_shares,
    toInt256(0)                                            AS swept,
    toInt256(0)                                            AS premium_shares,
    toInt256(0)                                            AS premium_offset_ray,
    toInt256(0)                                            AS deficit_ray,
    toInt64(sign)                                          AS events
FROM hub_events
WHERE event_name = 'MintFeeShares'

--@statement

-- Sweep: liquidity leaves for a reinvestment controller. Still the asset's, so
-- `swept` rises by what `liquidity` loses and `totalAddedAssets` counts both.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_sweep TO hub_assets AS
SELECT
    chain_id,
    address                                                AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))          AS asset_id,
    sign * -toInt256(JSONExtractString(body, 'amount'))    AS liquidity,
    toInt256(0)                                            AS added_shares,
    toInt256(0)                                            AS drawn_shares,
    sign * toInt256(JSONExtractString(body, 'amount'))     AS swept,
    toInt256(0)                                            AS premium_shares,
    toInt256(0)                                            AS premium_offset_ray,
    toInt256(0)                                            AS deficit_ray,
    toInt64(sign)                                          AS events
FROM hub_events
WHERE event_name = 'Sweep'

--@statement

-- Reclaim: swept liquidity comes back. The exact inverse of Sweep.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_reclaim TO hub_assets AS
SELECT
    chain_id,
    address                                                AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))          AS asset_id,
    sign * toInt256(JSONExtractString(body, 'amount'))     AS liquidity,
    toInt256(0)                                            AS added_shares,
    toInt256(0)                                            AS drawn_shares,
    sign * -toInt256(JSONExtractString(body, 'amount'))    AS swept,
    toInt256(0)                                            AS premium_shares,
    toInt256(0)                                            AS premium_offset_ray,
    toInt256(0)                                            AS deficit_ray,
    toInt64(sign)                                          AS events
FROM hub_events
WHERE event_name = 'Reclaim'

--@statement

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
WHERE event_name = 'RefreshPremium'

--@statement

-- UpdateAsset: the interest checkpoint, and the reason none of this needs an
-- RPC on the read path (§5.3). The emitted index is the *settled* one, because
-- `accrue()` writes `asset.drawnIndex` and `lastUpdateTimestamp` before
-- `updateDrawnRate` reads it to emit.
--
-- `block_timestamp` is the checkpoint's own time — the event carries none, and
-- without it the index cannot be extrapolated forward.
--
-- The ABI names the fourth parameter `accruedFees`; `AssetLogic:137` emits
-- `asset.realizedFees` into it. Same value, and the mirror uses the storage name.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_update_asset TO hub_asset_state AS
SELECT
    chain_id,
    address                                                       AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))                 AS asset_id,
    block_number,
    log_index,
    version,
    toUInt256(JSONExtractString(body, 'drawnIndex'))              AS drawn_index,
    toUInt256(JSONExtractString(body, 'drawnRate'))               AS drawn_rate,
    toUInt256(JSONExtractString(body, 'accruedFees'))             AS realized_fees,
    block_timestamp                                               AS index_timestamp,
    CAST(NULL, 'Nullable(UInt16)')                                AS liquidity_fee,
    CAST(NULL, 'Nullable(String)')                                AS underlying,
    CAST(NULL, 'Nullable(UInt8)')                                 AS decimals,
    sign
FROM hub_events
WHERE event_name = 'UpdateAsset'

--@statement

-- UpdateAssetConfig: only `liquidityFee` is read. It is the fee rate in the
-- supply side's `unrealizedFees` term (§5.2), so a wrong one moves every supply
-- valuation for the asset. The other three config fields are addresses that
-- nothing values.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_update_asset_config TO hub_asset_state AS
SELECT
    chain_id,
    address                                                        AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))                  AS asset_id,
    block_number,
    log_index,
    version,
    CAST(NULL, 'Nullable(UInt256)')                                AS drawn_index,
    CAST(NULL, 'Nullable(UInt256)')                                AS drawn_rate,
    CAST(NULL, 'Nullable(UInt256)')                                AS realized_fees,
    CAST(NULL, 'Nullable(DateTime(\'UTC\'))')                      AS index_timestamp,
    toUInt16(JSONExtractString(body, 'config', 'liquidityFee'))    AS liquidity_fee,
    CAST(NULL, 'Nullable(String)')                                 AS underlying,
    CAST(NULL, 'Nullable(UInt8)')                                  AS decimals,
    sign
FROM hub_events
WHERE event_name = 'UpdateAssetConfig'

--@statement

-- AddAsset: the ERC-20 address and its decimals, which appear in no other event
-- on either contract (§12.2). Seventeen of them on the Core Hub, all in one
-- block.
--
-- Lower-cased for the same reason every address is: the log's topic word is
-- lower-case and a caller reading a checksummed address off an explorer would
-- match nothing.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_add_asset TO hub_asset_state AS
SELECT
    chain_id,
    address                                            AS hub,
    toUInt256(JSONExtractString(body, 'assetId'))      AS asset_id,
    block_number,
    log_index,
    version,
    CAST(NULL, 'Nullable(UInt256)')                    AS drawn_index,
    CAST(NULL, 'Nullable(UInt256)')                    AS drawn_rate,
    CAST(NULL, 'Nullable(UInt256)')                    AS realized_fees,
    CAST(NULL, 'Nullable(DateTime(\'UTC\'))')          AS index_timestamp,
    CAST(NULL, 'Nullable(UInt16)')                     AS liquidity_fee,
    lower(JSONExtractString(body, 'underlying'))       AS underlying,
    toUInt8(JSONExtractString(body, 'decimals'))       AS decimals,
    sign
FROM hub_events
WHERE event_name = 'AddAsset'
