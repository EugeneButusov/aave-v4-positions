-- Emptied so `138`-`140` can refill it with the instants `133` left at the
-- epoch. 44,057 rows, replayed from a log that is retained in full.
--
-- The `listed_tokens` projection is part of the table and survives; it is
-- rebuilt from the rows that come back.
TRUNCATE TABLE hub_asset_state;
