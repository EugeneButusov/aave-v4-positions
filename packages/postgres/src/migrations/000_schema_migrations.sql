-- Records which migrations have been applied.
--
-- Applied unconditionally before anything else, because it cannot record its
-- own creation — the runner has to read this table to know what is pending, so
-- it must exist first. `IF NOT EXISTS` is what makes that safe to repeat.
--
-- A real primary key, where the ClickHouse ledger has only an ORDER BY: it lets
-- the runner record with ON CONFLICT DO NOTHING, so a re-run that races another
-- cannot leave two rows claiming the same migration.
CREATE TABLE IF NOT EXISTS schema_migrations (
    id         text        PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now()
);
