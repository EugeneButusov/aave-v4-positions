-- UpdateAsset: the interest checkpoint, and the reason none of this needs an
-- RPC on the read path (§5.3). The emitted index is the *settled* one, because
-- `accrue()` writes `asset.drawnIndex` and `lastUpdateTimestamp` before
-- `updateDrawnRate` reads it to emit.
--
-- `block_timestamp` is the checkpoint's own time — the event carries none, and
-- without it the index cannot be extrapolated forward.
--
-- The ABI names the fourth parameter `accruedFees`; `AssetLogic:137` emits
-- `asset.realizedFees` into it. Same value, and the fold uses the storage name.
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
WHERE event_name = 'UpdateAsset';
