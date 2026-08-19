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
WHERE event_name = 'UpdateAssetConfig';
