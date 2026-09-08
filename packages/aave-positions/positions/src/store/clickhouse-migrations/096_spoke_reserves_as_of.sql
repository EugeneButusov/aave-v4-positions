-- Which reserve pointed at which Hub asset, at a named instant.
--
-- The registry is latest-wins over `(block_number, log_index)`, so the cut
-- changes which `AddReserve` argMax lands on — and, for a reserve listed after
-- the instant, whether it lands on one at all. A reserve with no listing yet
-- returns no row, the join finds nothing, and the position reports a null
-- `asset` exactly as it does for a reserve the registry has never seen.
--
-- `sum(sign) > 0` still collapses, still groups on `version`, and the cut sits
-- outside it so a retraction travels with the row it retracts.
CREATE VIEW IF NOT EXISTS spoke_reserves_as_of AS
SELECT
    chain_id,
    spoke,
    reserve_id,
    argMax(asset_id, (block_number, log_index)) AS asset_id,
    argMax(hub, (block_number, log_index))      AS hub
FROM
(
    SELECT
        chain_id, spoke, reserve_id, block_number, log_index,
        any(asset_id) AS asset_id,
        any(hub)      AS hub
    FROM spoke_reserves
    WHERE block_timestamp <= {cut:DateTime}
    GROUP BY chain_id, spoke, reserve_id, block_number, log_index, version
    HAVING sum(sign) > 0
)
GROUP BY chain_id, spoke, reserve_id;
