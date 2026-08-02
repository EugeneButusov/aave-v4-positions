-- The additive half of the Hub asset mirror (§5.5).
--
-- Every field here is a **group under addition**, so retraction propagates for
-- free: the projection of a `sign = -1` ledger row is the exact negation of its
-- `+1` twin's, and the two sum to zero with no coordination and no ordering
-- assumption. That is the same property `user_positions` relies on, and it is
-- what decides which of the Hub's seventeen state fields can live here.
--
-- Verified against Hub.sol at commit 2524fe4 rather than taken from the
-- analysis's summary table, because one mis-folded transition silently corrupts
-- every supply valuation for that asset with no error (§5.5). Two things that
-- reading only the summary would have got wrong:
--
--   * `liquidity` is *assigned*, never incremented — `asset.liquidity =
--     liquidity.toUint120()`. It is additive anyway: the local is
--     `asset.liquidity ± delta` at all six call sites, and the `balanceOf` read
--     beside it is a solvency `require`, not the source of the value.
--   * `realizedFees` is **not** here. `accrue()` adds to it and
--     `_mintFeeShares` zeroes it, so it is not additive over the events — it is
--     latest-wins, and lives in `hub_asset_state`.
--
-- No `sign` column: values arrive already sign-multiplied, so a retraction is a
-- negative addend and the engine needs no notion of one.
CREATE TABLE IF NOT EXISTS hub_assets
(
    chain_id           UInt32,
    -- The emitting Hub. Asset ids are Hub-scoped, so this is part of the key
    -- rather than a filter — two Hubs both have an asset 7.
    hub                String,
    asset_id           UInt256,

    -- Underlying held by the Hub. Add/Reclaim/Restore raise it,
    -- Remove/Draw/Sweep lower it.
    liquidity          Int256,
    -- Supply-side shares. Add/MintFeeShares raise, Remove/EliminateDeficit lower.
    added_shares       Int256,
    -- Debt-side shares. Draw raises, Restore/ReportDeficit lower.
    drawn_shares       Int256,
    -- Sent to a reinvestment controller and not currently in `liquidity`.
    swept              Int256,

    -- The risk-premium pair, applied by Restore, ReportDeficit and
    -- RefreshPremium alike — `_validateApplyPremiumDelta` adds both deltas
    -- verbatim, which is what makes them additive.
    --
    -- `premium_offset_ray` is `int200` on chain and genuinely negative, which is
    -- the reason every column here is signed rather than merely wide.
    premium_shares     Int256,
    premium_offset_ray Int256,

    -- Bad debt, RAY-scaled. ReportDeficit raises, EliminateDeficit lowers.
    deficit_ray        Int256,

    -- sum(sign): additive, so it belongs here. Distinguishes an asset whose
    -- balances net to zero from one that was never touched.
    events             Int64
)
ENGINE = SummingMergeTree
PARTITION BY chain_id
ORDER BY (chain_id, hub, asset_id)
