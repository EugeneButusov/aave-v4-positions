-- Dropped so `position_liquidation_liquidator` can be recreated against a
-- `user_positions` that carries the instant each delta happened at. Recreated
-- at `077` under the same name, reading the same events.
--
-- Safe to leave the fold unwritten in between because nothing is writing: the
-- migrator is a one-shot the api and indexer wait on, which is what also makes
-- the backfill at `085` exact rather than double-counted.
DROP VIEW IF EXISTS position_liquidation_liquidator;
