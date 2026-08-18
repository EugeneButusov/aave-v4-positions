-- A fixture, not a real migration. It gives check_complete a directory to point
-- at, and is shaped like the files this crate will be handed: prose before the
-- first statement, which the splitter drops rather than send as a query.

CREATE TABLE IF NOT EXISTS fixture_one (id UInt8) ENGINE = MergeTree ORDER BY id;
