-- What Aave thinks each reserve is worth, one row per (chain, spoke, reserve).
--
-- **The second thing here that is not folded from the event log**, and it lands
-- beside `010_token_metadata.sql` for the same measured reason: a handful of
-- rows, point-looked-up, replaced whole. A `ReplacingMergeTree` carrying the
-- same data was measured at 67.05 ms per upsert against 1.97 ms, with the parts
-- piling up between merges. Prices only sharpen that — metadata is written once
-- and a price is rewritten every refresh, forever.
--
-- **A timestamp, and deliberately no block.** Everything else in this repository
-- is block-stamped because it is folded from an event log, where a block is the
-- only honest stamp. A price is fetched, and the next source to land may have no
-- block at all — a market API answers with a wall clock and nothing else.
-- Storing the block would bake one source's addressing into a table every source
-- has to share, and the column would go null the first time a second one landed.
-- `priced_at` is the stamp every source can produce, and it is the one §7.5 asks
-- for: HF carries the price *timestamp* so consumers can judge freshness. The
-- block a batch was pinned to still matters — it is what makes fourteen reads
-- agree with each other — but that is the adapter's business and stops there.
--
-- A second source becomes a `source` column in the primary key: one migration,
-- no data loss and no reshaping, which is the point of leaving the block out.
CREATE TABLE IF NOT EXISTS reserve_prices (
    -- bigint for the reason `indexer_cursor` gives: deployed chain ids already
    -- exceed int4, and EIP-2294 only caps them at MAX_SAFE_INTEGER.
    chain_id   bigint        NOT NULL,
    -- Lower-cased 0x-hex, enforced. Joined in the service against the `spoke`
    -- the position fold stores, which is `lower()`ed; a checksummed address
    -- written here would match nothing and read as a reserve nobody has priced.
    spoke      text          NOT NULL CHECK (spoke ~ '^0x[0-9a-f]{40}$'),
    -- numeric, not text. `reserveId` is a `uint256` on the wire, and numeric
    -- holds all 78 digits exactly while giving one canonical spelling per value
    -- — so there is no question of "13" and "0013" becoming two rows for one
    -- reserve. postgres.js hands it back as a string, which is what the port
    -- promises anyway.
    reserve_id numeric(78,0) NOT NULL,
    -- 8-decimal, per §7.4's ORACLE_DECIMALS, and `NOT NULL` on purpose — which
    -- inverts `token_metadata`'s rule rather than contradicting it. There a null
    -- *is* the token's answer, and the row records that the question was put.
    -- Here a refusal is never worth keeping: a failed read simply does not
    -- upsert, so the last good price survives and its age becomes visible
    -- instead of a blank appearing. A reserve never priced has no row at all.
    price      numeric(78,0) NOT NULL CHECK (price > 0),
    -- The database's own clock, so re-reading says when it was last asked
    -- rather than when some client thought it was.
    priced_at  timestamptz   NOT NULL DEFAULT now(),
    PRIMARY KEY (chain_id, spoke, reserve_id)
);
