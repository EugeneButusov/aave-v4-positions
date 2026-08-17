CREATE TABLE IF NOT EXISTS fixture_two (id UInt8, note String) ENGINE = MergeTree ORDER BY id;

CREATE VIEW IF NOT EXISTS fixture_two_current AS SELECT id, 'has ; inside' AS note FROM fixture_two;
