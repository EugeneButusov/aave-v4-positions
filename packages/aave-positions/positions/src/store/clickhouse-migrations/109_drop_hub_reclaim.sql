-- Dropped so `hub_reclaim` can be recreated against `hub_asset_deltas`,
-- which carries the instant each delta happened at. Recreated at `119`
-- under the same name, reading the same events.
DROP VIEW IF EXISTS hub_reclaim;
