-- Dropped so `hub_add` can be recreated against `hub_asset_deltas`,
-- which carries the instant each delta happened at. Recreated at `111`
-- under the same name, reading the same events.
DROP VIEW IF EXISTS hub_add;
