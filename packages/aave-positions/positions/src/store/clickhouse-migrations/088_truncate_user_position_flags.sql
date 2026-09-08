-- Emptied so `089` can refill it with the timestamps `086` left at the epoch.
--
-- Cheaper than it looks and cheaper than the alternative: 4,363 rows, replayed
-- from a log that is retained in full. An `ALTER TABLE … UPDATE` would have to
-- join `spoke_events` to find each row's instant, which is a mutation doing a
-- join to recover something the log already states plainly.
TRUNCATE TABLE user_position_flags;
