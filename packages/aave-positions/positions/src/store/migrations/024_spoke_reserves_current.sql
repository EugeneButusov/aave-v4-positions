-- The live registry entry per reserve: the same collapse-then-argMax shape as
-- the flag half of 023, for the same reasons.
CREATE VIEW IF NOT EXISTS spoke_reserves_current AS
SELECT
    chain_id,
    spoke,
    reserve_id,
    argMax(asset_id, (block_number, log_index)) AS asset_id,
    argMax(hub,      (block_number, log_index)) AS hub
FROM
(
    SELECT
        chain_id, spoke, reserve_id, block_number, log_index,
        any(asset_id) AS asset_id,
        any(hub)      AS hub
    FROM spoke_reserves
    GROUP BY chain_id, spoke, reserve_id, block_number, log_index, version
    HAVING sum(sign) > 0
)
GROUP BY chain_id, spoke, reserve_id
