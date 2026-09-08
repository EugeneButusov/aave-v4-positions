-- Dropped so `hub_restore` can be recreated against `hub_asset_deltas`,
-- which carries the instant each delta happened at. Recreated at `114`
-- under the same name, reading the same events.
DROP VIEW IF EXISTS hub_restore;
