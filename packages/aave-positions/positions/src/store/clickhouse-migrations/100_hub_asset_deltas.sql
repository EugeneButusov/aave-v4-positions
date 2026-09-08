-- The Hub asset balance sheet, at event grain.
--
-- **Why it is a second table rather than a change to `hub_assets`.** The page
-- joins this dimension whole and the join gives it nothing to prune by — no
-- predicate reaches it, because the Hub and asset are not known until the
-- registry resolves — so every page would read every row. Twenty rows became
-- 43,849 when measured, growing about 95k a year. `hub_assets` stays what it
-- was and becomes a rollup of this (`122`), so the common read is still twenty
-- rows and only a cut into the past pays for history.
--
-- `PARTITION BY` the month as well as the chain, which the position fold does
-- not need: there the wallet prefix prunes and here nothing does, so the
-- partition is the only pruning available. It is what makes the tail read in
-- `141` cheap — `cut = now()` touches one partition and finds nothing in it.
CREATE TABLE IF NOT EXISTS hub_asset_deltas
(
    chain_id           UInt32,
    hub                String,
    asset_id           UInt256,
    -- The block's own instant, carried from `hub_events`.
    block_timestamp    DateTime('UTC'),
    liquidity          Int256,
    added_shares       Int256,
    drawn_shares       Int256,
    swept              Int256,
    premium_shares     Int256,
    premium_offset_ray Int256,
    deficit_ray        Int256,
    events             Int64
)
ENGINE = SummingMergeTree
PARTITION BY (chain_id, toYYYYMM(block_timestamp))
ORDER BY (chain_id, hub, asset_id, block_timestamp);
