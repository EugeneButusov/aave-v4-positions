-- The reorg detector's retention window: finalityDepth + 1 rows per chain, 129
-- at the default depth, upserted by height and pruned from below.
--
-- A mutable set rather than a log, which is what decided the database. Every
-- iteration reads the whole window, upserts one row and deletes below a moving
-- floor; a rewind deletes above a point. Modelling that on an append-only
-- engine takes a version column, tombstone rows and a dedup read on the hot
-- path, where here it is three ordinary statements.
CREATE TABLE IF NOT EXISTS indexer_block_headers (
    chain_id        bigint NOT NULL,
    block_number    bigint NOT NULL CHECK (block_number >= 0),
    hash            text   NOT NULL CHECK (hash        ~ '^0x[0-9a-f]{64}$'),
    parent_hash     text   NOT NULL CHECK (parent_hash ~ '^0x[0-9a-f]{64}$'),
    -- Unix seconds as the block reported them, stored as the integer they are.
    -- timestamptz would add a format-and-reparse step on both sides whose only
    -- possible contribution is being wrong.
    block_timestamp bigint NOT NULL,

    -- This order and no other. It is what makes `append` an upsert — ON CONFLICT
    -- needs a unique index — and it is also the only index the prune and the
    -- truncate need: both are an equality on the leading column and a range on
    -- the trailing one, which is the case a composite btree exists for.
    -- Reversed, every prune would be a full scan.
    PRIMARY KEY (chain_id, block_number)
);
