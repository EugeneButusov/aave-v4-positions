-- Latest listing wins, over rows the collapse has already filtered — the same
-- shape as every other latest-wins read here, and ordered by chain order rather
-- than `version` for the same measured reason.
-- **Parameterised, and the parameter is the point.** A page is only as
-- point-in-time as its least point-in-time input, and a view that answered only
-- for now made `asOf` mean "value today's balances against a past index" — a
-- number that was never true at any block. Naming the instant is not a
-- convenience; it is what makes the answer well defined.
--
-- Here the cut decides whether a reserve resolves at all: one listed after the
-- instant has no `AddReserve` for `argMax` to land on, the join finds nothing,
-- and the position reports a null `asset` — the same answer as a reserve this
-- deployment has never seen.
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
