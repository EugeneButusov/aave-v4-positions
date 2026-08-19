-- The Spoke fold: 012 through 020, one projection each. Nine of them, because
-- one `LiquidationCall` log affects up to three positions and
-- `SetUsingAsCollateral` writes a different target.
--
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
