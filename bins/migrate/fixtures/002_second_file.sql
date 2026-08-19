-- Its only job is to be a second name in the directory.
CREATE TABLE IF NOT EXISTS fixture_two (id UInt8) ENGINE = MergeTree ORDER BY id;
