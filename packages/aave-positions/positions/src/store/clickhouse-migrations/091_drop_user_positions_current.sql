-- `user_positions_current` is gone; `090` answers for a named instant instead,
-- and `hub_assets_as_of(cut = now())` is how a caller that wants the newest
-- state asks for it.
--
-- Its two readers are the position store and the store's Rust port, both of
-- which already compute the instant they are valuing at and now pass it.
DROP VIEW IF EXISTS user_positions_current;
