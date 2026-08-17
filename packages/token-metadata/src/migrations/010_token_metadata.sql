-- What an ERC-20 calls itself, for the tokens the Hub has listed.
--
-- **The first thing here that is not folded from the event log.** No Aave event
-- carries a symbol (§12.5), so this is read from the token contract and stored
-- beside the ledger rather than derived from it. Three consequences, and the
-- table is shaped by all three: it has no `sign`, because there is nothing to
-- retract; it has no reorg story, because an address does not fork; and it can
-- be absent for a token that exists, which is why every label is nullable.
--
-- **Postgres, not ClickHouse, and it was measured rather than assumed.** One
-- row per (chain, token), seventeen of them, point-looked-up by address and
-- upserted whole. Against a `ReplacingMergeTree(fetched_at_block)` carrying the
-- same data, on the same host:
--
--   upsert the 17 rows          67.05 ms  ->   1.97 ms
--   read the dimension          4.39 ms   ->   0.27 ms   (FINAL vs primary key)
--   rows at rest after 103      51 in 3 parts  ->  17
--
-- The last line is the one that matters beyond speed. A column store answers
-- "replace this row" by writing another and collapsing later, so the read has
-- to say `FINAL` and the parts pile up between merges; `EXPLAIN indexes = 1`
-- shows the join reading all three. Postgres answers it in place. Emulating an
-- upsert took a version column, a `FINAL` and three paragraphs defending the
-- engine choice — that was the tell.
--
-- Serving it costs nothing extra: the API already reads Postgres on every
-- request for the indexer's cursor, and this read does not depend on the page,
-- so it runs beside the ClickHouse query rather than after it. Measured at
-- -1.44 ms against a 28.00 ms baseline page — noise, which is the point.
CREATE TABLE IF NOT EXISTS token_metadata (
    -- bigint for the reason `indexer_cursor` gives: deployed chain ids already
    -- exceed int4, and EIP-2294 only caps them at MAX_SAFE_INTEGER.
    chain_id         bigint      NOT NULL,
    -- Lower-cased 0x-hex, enforced. It is joined against
    -- `hub_assets_current.underlying`, which the fold stores `lower()`ed; a
    -- checksummed address written here would match nothing and read as a token
    -- nobody has enriched yet, forever.
    token            text        NOT NULL CHECK (token ~ '^0x[0-9a-f]{40}$'),
    -- All three nullable, because all three are OPTIONAL in EIP-20 and a token
    -- that implements none of them is unusual but conformant. NULL here means
    -- "asked, and there is no answer" — the row existing is what records that
    -- the question was put. A missing row means it has not been asked yet, and
    -- keeping those two apart is what stops a mute token being re-read forever.
    symbol           text,
    name             text,
    -- The token's own, and deliberately not what anything is scaled by: the
    -- Hub's `AddAsset` carries its own decimals, that is what the Hub's
    -- arithmetic uses, and that is what a position is valued with. This column
    -- exists so the two can be compared — a disagreement is a listing audit
    -- signal, not a correctness problem for our numbers.
    token_decimals   smallint    CHECK (token_decimals BETWEEN 0 AND 255),
    fetched_at       timestamptz NOT NULL DEFAULT now(),
    -- The block every field was read at. One pinned height per sweep, so three
    -- calls for one token cannot land at three heights across a provider
    -- failover list.
    fetched_at_block bigint      NOT NULL,
    PRIMARY KEY (chain_id, token)
);
