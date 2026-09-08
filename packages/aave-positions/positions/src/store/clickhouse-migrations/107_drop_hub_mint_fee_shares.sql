-- Dropped so `hub_mint_fee_shares` can be recreated against `hub_asset_deltas`,
-- which carries the instant each delta happened at. Recreated at `117`
-- under the same name, reading the same events.
DROP VIEW IF EXISTS hub_mint_fee_shares;
