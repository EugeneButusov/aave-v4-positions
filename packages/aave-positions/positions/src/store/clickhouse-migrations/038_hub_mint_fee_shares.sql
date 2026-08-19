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
WHERE event_name = 'MintFeeShares';
