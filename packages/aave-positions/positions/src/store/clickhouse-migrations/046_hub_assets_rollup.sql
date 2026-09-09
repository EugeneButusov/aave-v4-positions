-- `030_hub_assets` filled from `028_hub_asset_deltas`.
--
-- One statement, and it is what makes the rollup safe to trust: the totals are
-- derived from the deltas rather than written beside them, so no edit to one of
-- the ten projections can leave the two disagreeing. `SummingMergeTree` does the
-- rest — the target is keyed by asset with no time in it, so every delta for an
-- asset merges into its one row.
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
