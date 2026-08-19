-- The Hub fold: 032 through 044. Ten fold additive asset state into
-- `hub_assets`, three record a latest-wins value into `hub_asset_state`. §5.5
-- calls it the highest-risk fold in the design: one mishandled transition
-- silently corrupts every supply valuation for that asset, with no error and
-- nothing to flag it.
--
-- **The transitions are transcribed from Hub.sol at commit 2524fe4**, not from
-- the analysis's summary table. Three of them the analysis does not cover at
-- all, and a fourth it states in a way that would mislead — see the comments on
-- EliminateDeficit, RefreshPremium, MintFeeShares and Add.
--
-- The three invariants from `012_position_supply.sql` hold in every one and are
-- not repeated: read the table and never the view, multiply every value by the
-- source `sign`, and extract through JSONExtractString + toInt256 rather than
-- JSONExtractInt, which returns 0 above Int64 max.
--
-- `topic1` is the asset id on all thirteen, but it is read out of `body`
-- alongside everything else so one expression style covers them.
--
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
WHERE event_name = 'Add';
