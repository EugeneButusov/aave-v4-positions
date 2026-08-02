-- The additive half of a position, pre-aggregated.
--
-- Every column here is a group under addition, which is the whole reason the
-- fold needs no coordination with the indexer. The ledger retracts a range by
-- writing its negation; each materialized view multiplies by the source row's
-- `sign`; so a retraction arrives as a negative addend and the sums net out.
-- Idempotence and reorg repair are the same mechanism, and neither one is code.
--
-- What is deliberately *not* here: `using_as_collateral`, which is latest-wins
-- rather than additive. No engine can hold that pre-aggregated under
-- retraction — measured, see user_position_flags — so it lives at event grain
-- and is resolved at read time.
--
-- No `sign` column: values arrive already sign-multiplied, so the engine needs
-- no notion of one.
CREATE TABLE IF NOT EXISTS user_positions
(
    chain_id            UInt32,
    -- Lower-cased. Addresses decoded out of `body` are checksummed, so a caller
    -- passing the lower-case form they got from a URL would otherwise match
    -- nothing. §12.1: the key is `user`, never `caller`.
    user                String,
    -- The emitting Spoke. Part of the key rather than a filter: reserves are
    -- Spoke-scoped, and the same underlying on two Spokes is two positions with
    -- independent risk config and independent health factors (§12.3).
    spoke               String,
    reserve_id          UInt256,
    -- Signed because withdrawals and repayments subtract, and Int256 rather
    -- than something that "looks big enough" — shares are uint120 on chain and
    -- §7.5 is about exactly that habit.
    supplied_shares     Int256,
    drawn_shares        Int256,
    premium_shares      Int256,
    premium_offset_ray  Int256,
    -- Net principal flow in asset units, which is NOT a balance: between events
    -- the Hub's index accrues and emits nothing (§5). Reportable as "net
    -- deposited", not as "worth".
    net_supplied_amount Int256,
    net_borrowed_amount Int256,
    -- sum(sign) — additive, so it belongs here rather than being a count() over
    -- the ledger. Also what keeps a closed position visible: its shares net to
    -- zero but its history did happen.
    events              Int64
)
ENGINE = SummingMergeTree
-- Not by block. The key leads with `user`, so a block-range partition would
-- scatter one wallet across every partition; nothing reads this by range.
PARTITION BY chain_id
-- Leads with `user` because the access pattern is "positions of this wallet",
-- where the ledger's is "logs in this block range". Different table, different
-- sorting key.
ORDER BY (chain_id, user, spoke, reserve_id)
