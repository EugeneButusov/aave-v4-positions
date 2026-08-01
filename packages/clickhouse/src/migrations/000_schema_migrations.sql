-- Records which migrations have been applied.
--
-- Applied unconditionally before anything else, because it cannot record its
-- own creation — the runner has to read this table to know what is pending, so
-- it must exist first. `IF NOT EXISTS` is what makes that safe to repeat.
CREATE TABLE IF NOT EXISTS schema_migrations
(
    id         String,
    applied_at DateTime64(3, 'UTC') DEFAULT now64(3)
)
ENGINE = MergeTree
ORDER BY id
