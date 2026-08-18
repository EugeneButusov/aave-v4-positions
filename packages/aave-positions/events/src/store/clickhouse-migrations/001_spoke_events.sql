-- Aave v4 Spoke position events, append-only.
--
-- There is no DELETE and no mutation anywhere against this table: a reorg
-- retracts a row by writing its negation, which is the same mechanism a retry
-- uses, so there is one write path to reason about.
--
-- One shape for all eight events. Only three of them share a field layout, so a
-- column per field would be mostly nulls and would need a migration per new
-- event; instead the indexed parameters stay the topic words they arrived as
-- and everything else is JSON.
CREATE TABLE IF NOT EXISTS spoke_events
(
    chain_id        UInt32,
    -- The emitting Spoke. One per processor today, so every row in a given
    -- deployment carries the same value — but stored, not assumed, so a second
    -- Spoke writing here stays distinguishable.
    address         String,
    block_number    UInt64,
    log_index       UInt32,
    -- One value per dispatch. Pairing is on (sorting key, version), so a
    -- retraction has to carry the version of the row it cancels.
    version         UInt64,
    block_hash      String,
    block_timestamp DateTime('UTC'),
    tx_hash         String,
    tx_index        UInt32,
    event_name      LowCardinality(String),
    -- The indexed parameters, in ABI order. topic0 is the signature hash, which
    -- event_name already says.
    --
    -- Only topic1 means the same thing across all eight: the reserve id, or the
    -- collateral one on LiquidationCall. topic2 is caller, user, debtReserveId
    -- or assetId depending on the event; topic3 is user or hub, and is null on
    -- ReportDeficit, which indexes only two. Reading either without filtering
    -- on event_name returns the wrong field rather than nothing, which is what
    -- spoke_events_current's per-event consumers exist to prevent.
    topic1          Nullable(String),
    topic2          Nullable(String),
    topic3          Nullable(String),
    -- Every decoded argument, indexed and not, each uint256 already a string:
    -- narrowing one to a number because it looks small is the habit §7.5 is
    -- about.
    body            String,
    -- The raw non-indexed bytes, kept beside the decoded body so a decoder bug
    -- is repairable by re-decoding what is already stored instead of
    -- re-fetching the backfill.
    data            String,
    -- +1 live, -1 retraction.
    sign            Int8
)
ENGINE = VersionedCollapsingMergeTree(sign, version)
PARTITION BY intDiv(block_number, 1000000)
-- log_index is unique within a block — verified against a mainnet block whose
-- 838 logs ran 0..837 unbroken across transaction boundaries — so tx_hash would
-- add no uniqueness, and placed ahead of log_index it would order a block's
-- logs by a random digest and destroy replay order.
ORDER BY (chain_id, block_number, log_index);
