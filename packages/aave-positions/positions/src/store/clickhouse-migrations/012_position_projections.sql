-- Every projection of the ledger, in one change.
--
-- Nine views, because one `LiquidationCall` log affects up to three positions
-- and `SetUsingAsCollateral` writes a different target. They are one file
-- because they are one thing: the fold. Adding an event means adding a section
-- here, and reviewing the fold means reading this rather than nine diffs.
--
-- Each statement ends in `;`, which is what the runner splits on, and what lets
-- this file be pasted into a console unchanged. ClickHouse's HTTP interface
-- refuses multi-statement requests, so something has to take the file apart; the
-- splitter skips semicolons inside comments and quoted text, so the prose is safe.
-- Every statement is `IF NOT EXISTS`, because a file that fails part-way through
-- is recorded nowhere and retried from the top.

-- Supply: shares in, assets in. The first projection, and the one whose
-- comments the rest do not repeat — three invariants hold in every one:
--
--  1. FROM spoke_events, never spoke_events_current. A materialized view is an
--     insert trigger and nothing inserts into a plain view — measured, the DDL
--     is accepted and the view then never fires, silently and forever.
--  2. Every value is multiplied by the source row's `sign`. The ledger's
--     retraction is a full-column copy, so its projection is the exact negation
--     of the original's and the two sum to zero. That single multiplication is
--     the entire reorg and idempotence story.
--  3. Values come out of `body` as strings and go through toInt256, never
--     JSONExtractInt. Measured: JSONExtractInt returns *0* above Int64 max, not
--     a truncation — so it is correct for small positions and silently zeroes
--     large ones, which is the worst shape a bug can have. toInt256 throws on a
--     value it cannot parse, so an unexpected body fails the insert instead.
--
-- Addresses are lower-cased because viem hands back checksummed ones
-- (0xa0F5c2Bb…), and a caller querying with the lower-case form would match
-- nothing. The position keys on `user`, never `caller` (§2).
CREATE MATERIALIZED VIEW IF NOT EXISTS position_supply TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                      AS user,
    address                                                     AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))             AS reserve_id,
    sign * toInt256(JSONExtractString(body, 'suppliedShares'))  AS supplied_shares,
    toInt256(0)                                                 AS drawn_shares,
    toInt256(0)                                                 AS premium_shares,
    toInt256(0)                                                 AS premium_offset_ray,
    sign * toInt256(JSONExtractString(body, 'suppliedAmount'))  AS net_supplied_amount,
    toInt256(0)                                                 AS net_borrowed_amount,
    toInt64(sign)                                               AS events
FROM spoke_events
WHERE event_name = 'Supply';

-- Withdraw: shares out, assets out. See position_supply for the three invariants.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_withdraw TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                       AS user,
    address                                                      AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))              AS reserve_id,
    sign * -toInt256(JSONExtractString(body, 'withdrawnShares')) AS supplied_shares,
    toInt256(0)                                                  AS drawn_shares,
    toInt256(0)                                                  AS premium_shares,
    toInt256(0)                                                  AS premium_offset_ray,
    sign * -toInt256(JSONExtractString(body, 'withdrawnAmount')) AS net_supplied_amount,
    toInt256(0)                                                  AS net_borrowed_amount,
    toInt64(sign)                                                AS events
FROM spoke_events
WHERE event_name = 'Withdraw';

-- Borrow: debt shares in, assets out to the borrower. See position_supply.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_borrow TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                    AS user,
    address                                                   AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))           AS reserve_id,
    toInt256(0)                                               AS supplied_shares,
    sign * toInt256(JSONExtractString(body, 'drawnShares'))   AS drawn_shares,
    toInt256(0)                                               AS premium_shares,
    toInt256(0)                                               AS premium_offset_ray,
    toInt256(0)                                               AS net_supplied_amount,
    sign * toInt256(JSONExtractString(body, 'drawnAmount'))   AS net_borrowed_amount,
    toInt64(sign)                                             AS events
FROM spoke_events
WHERE event_name = 'Borrow';

-- Repay: debt shares out, plus the premium components. See position_supply.
--
-- `premiumDelta` is an int256 triple and is summed exactly as emitted — it is a
-- delta, so it adds. Nothing on-chain confirms that reading: the premium has
-- never been non-zero on mainnet (§5.4), so the sign convention is pinned by a
-- synthetic spec and the reconciliation cannot check it.
--
-- The six-field shape is Repay's alone: it inserts `totalAmountRepaid` where
-- the other three hot events carry a plain `amount`.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_repay TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                        AS user,
    address                                                                       AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))                               AS reserve_id,
    toInt256(0)                                                                   AS supplied_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnShares'))                      AS drawn_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))       AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta'))    AS premium_offset_ray,
    toInt256(0)                                                                   AS net_supplied_amount,
    sign * -toInt256(JSONExtractString(body, 'totalAmountRepaid'))                AS net_borrowed_amount,
    toInt64(sign)                                                                 AS events
FROM spoke_events
WHERE event_name = 'Repay';

-- ReportDeficit: bad debt written off the borrower. See position_supply.
--
-- §4.1's trap 2. When collateral is exhausted and debt remains, the Spoke emits
-- this alongside the LiquidationCall, and *this* is what removes the written-off
-- shares. Folding only LiquidationCall leaves the borrower carrying phantom debt
-- forever.
--
-- No asset-amount leg: the event carries shares and the premium triple, and
-- nothing was transferred. It has never fired on mainnet, so like the premium it
-- is pinned by a synthetic spec rather than by reconciliation.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_report_deficit TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                     AS user,
    address                                                                    AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))                            AS reserve_id,
    toInt256(0)                                                                AS supplied_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnShares'))                   AS drawn_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))    AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta')) AS premium_offset_ray,
    toInt256(0)                                                                AS net_supplied_amount,
    toInt256(0)                                                                AS net_borrowed_amount,
    toInt64(sign)                                                              AS events
FROM spoke_events
WHERE event_name = 'ReportDeficit';

-- LiquidationCall, leg 1 of 3: collateral seized from the borrower.
--
-- One log affects up to three positions, so it gets three projections rather
-- than one arrayJoin — each stays readable and each fails on its own.
--
-- The seized reserve is `collateralReserveId`, which is topic1. Self-liquidation
-- — USDC debt against USDC collateral — puts this leg and the next on the same
-- key, and SummingMergeTree simply adds them into one row carrying both deltas.
-- That is correct, and it is why no leg discriminator column exists; under a
-- collapsing engine it would have needed one.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_liquidation_collateral TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                   AS user,
    address                                                                  AS spoke,
    toUInt256(JSONExtractString(body, 'collateralReserveId'))                AS reserve_id,
    sign * -toInt256(JSONExtractString(body, 'collateralSharesLiquidated'))  AS supplied_shares,
    toInt256(0)                                                              AS drawn_shares,
    toInt256(0)                                                              AS premium_shares,
    toInt256(0)                                                              AS premium_offset_ray,
    sign * -toInt256(JSONExtractString(body, 'collateralAmountRemoved'))     AS net_supplied_amount,
    toInt256(0)                                                              AS net_borrowed_amount,
    toInt64(sign)                                                            AS events
FROM spoke_events
WHERE event_name = 'LiquidationCall';

-- LiquidationCall, leg 2 of 3: debt restored on the borrower.
--
-- A different reserve from leg 1 — `debtReserveId`, which is topic2 — so this is
-- a different position for the same user, and the two legs of one liquidation
-- land on two rows. See position_liquidation_collateral for the self-liquidation
-- case where they do not.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_liquidation_debt TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                     AS user,
    address                                                                    AS spoke,
    toUInt256(JSONExtractString(body, 'debtReserveId'))                        AS reserve_id,
    toInt256(0)                                                                AS supplied_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnSharesLiquidated'))         AS drawn_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))    AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta')) AS premium_offset_ray,
    toInt256(0)                                                                AS net_supplied_amount,
    sign * -toInt256(JSONExtractString(body, 'debtAmountRestored'))            AS net_borrowed_amount,
    toInt64(sign)                                                              AS events
FROM spoke_events
WHERE event_name = 'LiquidationCall';

-- LiquidationCall, leg 3 of 3: collateral credited to the liquidator.
--
-- §4.1's trap 3, and the reason this view exists at all: when `receiveShares`
-- is true the collateral never leaves the Hub, ownership moves inside the Spoke,
-- and the liquidator's position grows with **no Supply event anywhere in the
-- trace**. Crediting supplied shares only on Supply silently under-counts every
-- liquidator, with nothing in the logs to indicate a problem.
--
-- It has never fired on mainnet — 0 of 90 liquidations — which is the argument
-- for folding it now rather than later: it cannot be tested against production
-- data when it first appears, and it is cheap while the transition table is
-- fresh. A synthetic spec is the only proof it will ever have.
--
-- No asset-amount leg: nothing was transferred, so `collateralAmountRemoved`
-- belongs to leg 1 alone. The gap between `collateralSharesLiquidated` and
-- `collateralSharesToLiquidator` is the protocol fee, which settles as a Hub
-- TransferShares to the treasury spoke and touches no user position.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_liquidation_liquidator TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'liquidator'))                              AS user,
    address                                                                   AS spoke,
    toUInt256(JSONExtractString(body, 'collateralReserveId'))                 AS reserve_id,
    sign * toInt256(JSONExtractString(body, 'collateralSharesToLiquidator'))  AS supplied_shares,
    toInt256(0)                                                               AS drawn_shares,
    toInt256(0)                                                               AS premium_shares,
    toInt256(0)                                                               AS premium_offset_ray,
    toInt256(0)                                                               AS net_supplied_amount,
    toInt256(0)                                                               AS net_borrowed_amount,
    toInt64(sign)                                                             AS events
FROM spoke_events
WHERE event_name = 'LiquidationCall' AND JSONExtractBool(body, 'receiveShares');

-- SetUsingAsCollateral -> user_position_flags.
--
-- The one projection that copies `version` and `sign` verbatim instead of
-- folding the sign into a value: this target collapses rather than sums, so the
-- retraction has to arrive as the pair's other half. See 011 for why the flag
-- cannot live in user_positions at all.
--
-- `AddReserve` has no projection. It is captured in the ledger like the other
-- seven, but nothing reads reserveId -> (assetId, hub) until Hub ingestion gives
-- assetId a meaning, and a projection built in advance of its first consumer is
-- one more thing to keep correct for no reader.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_collateral_flag TO user_position_flags AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))          AS user,
    address                                         AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId')) AS reserve_id,
    block_number,
    log_index,
    version,
    JSONExtractBool(body, 'usingAsCollateral')      AS using_as_collateral,
    sign
FROM spoke_events
WHERE event_name = 'SetUsingAsCollateral';
