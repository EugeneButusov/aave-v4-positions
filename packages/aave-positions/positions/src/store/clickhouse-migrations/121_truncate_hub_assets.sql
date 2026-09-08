-- Emptied so `122` can refill it from the deltas.
--
-- Nothing writes to it directly any more: `101`-`110` dropped the ten views that
-- did, and `122` makes it a rollup of `hub_asset_deltas` instead. Twenty rows,
-- and the events behind them are all still in `hub_events`.
TRUNCATE TABLE hub_assets;
