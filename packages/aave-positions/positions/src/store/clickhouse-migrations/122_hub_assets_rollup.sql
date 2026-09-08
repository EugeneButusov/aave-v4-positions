-- `hub_assets` becomes a rollup of `hub_asset_deltas`.
--
-- **Derived, not written in parallel.** Two tables holding the same facts drift
-- the moment one projection is edited and the other is not; this one cannot,
-- because it has no source but the deltas. Every column is an additive signed
-- delta, so summing them by asset is the same arithmetic the old views did — the
-- rollup was verified row-for-row against the table it replaces, all 17 assets,
-- before this landed.
--
-- Created before the backfills at `123`-`132`, so their inserts fire it and it
-- fills from them.
CREATE MATERIALIZED VIEW IF NOT EXISTS hub_assets_rollup TO hub_assets AS
SELECT
    chain_id,
    hub,
    asset_id,
    liquidity,
    added_shares,
    drawn_shares,
    swept,
    premium_shares,
    premium_offset_ray,
    deficit_ray,
    events
FROM hub_asset_deltas;
