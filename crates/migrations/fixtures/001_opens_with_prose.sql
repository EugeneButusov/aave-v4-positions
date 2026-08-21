-- A fixture, not a real migration. It gives check_complete a directory to point
-- at, and is shaped like the files this crate is handed: prose, then the one
-- statement the file holds.
CREATE TABLE IF NOT EXISTS fixture_one (id UInt8) ENGINE = MergeTree ORDER BY id;
