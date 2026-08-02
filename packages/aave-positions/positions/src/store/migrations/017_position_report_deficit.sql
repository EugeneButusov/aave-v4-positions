-- ReportDeficit: bad debt written off the borrower. See 013.
--
-- §4.1's trap 2. When collateral is exhausted and debt remains, the Spoke emits
-- this alongside the LiquidationCall, and *this* is what removes the written-off
-- shares. Folding only LiquidationCall leaves the borrower carrying phantom debt
-- forever.
--
-- No asset-amount leg: the event carries shares and the premium triple, and
-- nothing was transferred. It has never fired on mainnet, so like the premium it
-- is pinned by a synthetic spec rather than by reconciliation.
CREATE MATERIALIZED VIEW IF NOT EXISTS position_report_deficit TO user_positions AS
SELECT
    chain_id,
    lower(JSONExtractString(body, 'user'))                                     AS user,
    address                                                                    AS spoke,
    toUInt256(JSONExtractString(body, 'reserveId'))                            AS reserve_id,
    toInt256(0)                                                                AS supplied_shares,
    sign * -toInt256(JSONExtractString(body, 'drawnShares'))                   AS drawn_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'sharesDelta'))    AS premium_shares,
    sign * toInt256(JSONExtractString(body, 'premiumDelta', 'offsetRayDelta')) AS premium_offset_ray,
    toInt256(0)                                                                AS net_supplied_amount,
    toInt256(0)                                                                AS net_borrowed_amount,
    toInt64(sign)                                                              AS events
FROM spoke_events
WHERE event_name = 'ReportDeficit'
