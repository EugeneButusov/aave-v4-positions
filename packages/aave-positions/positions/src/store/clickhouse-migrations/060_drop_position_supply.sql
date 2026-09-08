-- Dropped so `position_supply` can be recreated against a
-- `user_positions` that carries the instant each delta happened at. Recreated
-- at `070` under the same name, reading the same events.
--
-- Safe to leave the fold unwritten in between because nothing is writing: the
-- migrator is a one-shot the api and indexer wait on, which is what also makes
-- the backfill at `078` exact rather than double-counted.
DROP VIEW IF EXISTS position_supply;
