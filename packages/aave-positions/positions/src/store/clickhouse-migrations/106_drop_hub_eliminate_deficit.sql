-- Dropped so `hub_eliminate_deficit` can be recreated against `hub_asset_deltas`,
-- which carries the instant each delta happened at. Recreated at `116`
-- under the same name, reading the same events.
DROP VIEW IF EXISTS hub_eliminate_deficit;
