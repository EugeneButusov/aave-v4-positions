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
WHERE event_name = 'AddAsset';
