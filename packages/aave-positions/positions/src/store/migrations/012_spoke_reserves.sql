-- reserveId -> (assetId, hub), from AddReserve. Fourteen rows over all history.
--
-- Note what is absent: the underlying ERC-20 address. `AddReserve(reserveId,
-- assetId, hub)` indexes all three of its parameters and carries no
-- `underlying` — checked against ISpokeV4_ABI, not assumed. The token address
-- lives on the Hub's `AddAsset(assetId, underlying, decimals)`, so it arrives
-- with Hub ingestion and not before.
--
-- Event grain and the same collapsing engine as the flags, for the same reason:
-- "the current registry entry" is latest-wins, so a retraction has to remove a
-- generation rather than a key.
CREATE TABLE IF NOT EXISTS spoke_reserves
(
    chain_id     UInt32,
    spoke        String,
    reserve_id   UInt256,
    block_number UInt64,
    log_index    UInt32,
    version      UInt64,
    asset_id     UInt256,
    hub          String,
    sign         Int8
)
ENGINE = VersionedCollapsingMergeTree(sign, version)
PARTITION BY chain_id
ORDER BY (chain_id, spoke, reserve_id, block_number, log_index)
