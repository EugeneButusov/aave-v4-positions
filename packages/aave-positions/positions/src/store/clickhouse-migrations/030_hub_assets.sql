-- The additive half of the Hub asset fold (§5.5), at event grain.
--
-- **One row per event, not per asset**, for the reason `010_user_positions`
-- gives: a total is the sum of every delta up to a moment, and without a moment
-- to sum up to it can only answer for now. Summing on write answers that
-- question once, for the wrong instant, and leaves nothing to re-sum.
--
-- A split was tried — deltas here, a rollup keyed by asset beside them, a past
-- instant served as `rollup − everything after the cut` — on the theory that a
-- page reads this dimension whole with nothing to prune by, so event grain would
-- cost every delta ever written on every page. Measured at real scale it cost
-- nothing: reading the deltas outright is *faster* at every cut, because the
-- page already reads all of `hub_asset_state` either way and the extra columnar
-- scan is cheaper than the third union branch and the rollup's own `GROUP BY`.
-- The rollup bought a table, a view, a subtraction and an invariant to keep, for
-- 3-4ms of loss. What actually dominates this read is the `argMax` over
-- `hub_asset_state`, which is the materialisation `design-notes` already has
-- queued.
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
    -- The block's own instant, in the position the ten projections write it.
    -- Every other column here is a signed delta, so a cut is membership and
    -- nothing has to be re-derived to answer for a past moment.
    --
    -- **Reorg-safe, and it rests on something outside this file.** A retraction
    -- is the same log written back with `sign = -1`, so its projection is the
    -- exact negation of the `+1` twin's and the two sum to zero. Putting the
    -- instant in the sorting key adds a condition: the pair must share it, or
    -- they land on different rows and a cut between them keeps a delta the chain
    -- no longer has. They do, because `revert` is `INSERT … SELECT` over the
    -- ledger's own rows and `block_timestamp` is among the columns it copies. If
    -- that stops being true, this table, `user_positions` and `hub_asset_state`
    -- all go quietly wrong at past instants and stay right at now — three cases
    -- in `position-valuation.spec.ts` fail if it does.
    block_timestamp    DateTime('UTC'),

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
-- By month as well as chain, which the position fold does not need: there the
-- wallet prefix prunes a page down to one wallet's rows and here nothing does,
-- so the partition is the only pruning a cut can use.
PARTITION BY (chain_id, toYYYYMM(block_timestamp))
ORDER BY (chain_id, hub, asset_id, block_timestamp);
