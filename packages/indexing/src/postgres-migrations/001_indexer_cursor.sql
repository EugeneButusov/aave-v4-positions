-- How far indexing has got, one row per chain, upserted in place.
--
-- Not a journal. The only question ever asked of this table is "where do I
-- resume", asked once per process start, so history would be rows nothing reads
-- written on the one path that sits inline in the loop's critical path — about
-- 932 back-to-back writes for a full catch-up, one per block time at the tip.
--
-- The update is HOT: no indexed column changes, so the dead tuple is reclaimed
-- inside the page and a one-row table stays a one-row table.
CREATE TABLE IF NOT EXISTS indexer_cursor (
    -- bigint, not integer. Deployed chain ids already exceed int4 — Palm is
    -- 11297108109 — and EIP-2294 only caps them at MAX_SAFE_INTEGER. An
    -- overflow here is an insert that fails on the first chain nobody tested.
    chain_id   bigint      PRIMARY KEY,
    last_block bigint      NOT NULL CHECK (last_block >= 0),
    -- Lowercase 0x-hex, enforced. The detector compares hashes with `!==`, so a
    -- differently-cased hash would never match anything and would read as a
    -- permanent, unrecoverable reorg. Refusing the write makes that a loud
    -- failure at the moment it happens instead of a wrong answer later.
    last_hash  text        NOT NULL CHECK (last_hash ~ '^0x[0-9a-f]{64}$'),
    -- Nothing reads this. It is for the operator answering "is this indexer
    -- alive?" from psql when the process itself is not reachable.
    updated_at timestamptz NOT NULL DEFAULT now()
)
