-- The additive half of the Hub asset fold, at event grain.
--
-- **Why the Hub's totals are split in two where the position fold is not.** A
-- page reads one wallet's positions, so the wallet prefix prunes and event grain
-- costs a few hundred rows. It reads the Hub dimension *whole* — the asset is
-- not known until the registry resolves, so no predicate reaches that side of
-- the join — and event grain there would be every delta ever written, on every
-- page: 43,849 rows to produce seventeen when measured, growing about 95k a
-- year.
--
-- So the deltas live here and `030_hub_assets` becomes a rollup of them
-- (`046`), keyed by asset with no time in it. A read at now touches the rollup
-- and nothing else; a read at a past instant subtracts the tail from it, which
-- `045` explains. That keeps the common case at twenty rows and makes only
-- history cost anything.
--
-- Partitioned by month as well as chain, which the position fold does not need:
-- there the wallet prefix prunes and here nothing does, so the partition is the
-- only pruning available — and it is what makes `045`'s tail read cheap.
--
-- The columns are `030`'s exactly and mean what that file says they mean; it is
-- where they are argued from Hub.sol and where they should stay argued.
--
-- **Reorg-safe, and it rests on something outside this file.** A retraction is
-- the same log written back with `sign = -1`, so its projection is the exact
-- negation of the `+1` twin's and the two sum to zero — that much is the same
-- property the fold has always had. Putting `block_timestamp` in the sorting
-- key adds a condition: the pair must share it, or they land on different rows
-- and a cut between them keeps a delta the chain no longer has.
--
-- They do, because `revert` is `INSERT … SELECT` over the ledger's own rows and
-- `block_timestamp` is among the columns it copies. If that ever stops being
-- true — a retraction generated from the replacement branch rather than from
-- the ledger — this table, `user_positions` and `hub_asset_state` all go quietly
-- wrong at past instants and stay right at now, which is the hard way to find
-- out. Three cases in `position-valuation.spec.ts` fail if it does.
CREATE TABLE IF NOT EXISTS hub_asset_deltas
(
    chain_id           UInt32,
    hub                String,
    asset_id           UInt256,
    liquidity          Int256,
    added_shares       Int256,
    drawn_shares       Int256,
    swept              Int256,
    premium_shares     Int256,
    premium_offset_ray Int256,
    deficit_ray        Int256,
    events             Int64,
    -- The block's own instant. Every column above is a signed delta, so a cut is
    -- membership and nothing has to be re-derived to answer for a past moment.
    block_timestamp    DateTime('UTC')
)
ENGINE = SummingMergeTree
PARTITION BY (chain_id, toYYYYMM(block_timestamp))
ORDER BY (chain_id, hub, asset_id, block_timestamp);
