-- Aave v4 Hub asset-state events, append-only.
--
-- Same shape, same engine and same write path as `spoke_events` — and
-- deliberately not the same table.
--
-- `ReportDeficit` exists on both contracts with different signatures and
-- different topic0s (§4.4, verified). The position projections filter on
-- `event_name` with no address predicate, so a Hub `ReportDeficit` row landing
-- in `spoke_events` would fire a view that extracts a `user` field the Hub form
-- does not have. A materialized view that throws fails the insert while the
-- ledger row still commits — measured — so Hub ingestion would jam and the
-- position fold would be left short by exactly that event. `Add`, `Remove` and
-- `Draw` are generically named for the same reason (§4.5).
--
-- `topic1` settles it independently: it is the **asset id** here and a reserve
-- id there. One column cannot hold two meanings and stay queryable.
CREATE TABLE IF NOT EXISTS hub_events
(
    chain_id        UInt32,
    -- The emitting Hub. One per processor today, so every row in a given
    -- deployment carries the same value — but stored, not assumed, so a second
    -- Hub writing here stays distinguishable.
    address         String,
    block_number    UInt64,
    log_index       UInt32,
    -- One value per dispatch. Pairing is on (sorting key, version), so a
    -- retraction has to carry the version of the row it cancels.
    version         UInt64,
    block_hash      String,
    -- Not decoration: `UpdateAsset` carries no timestamp of its own, and the
    -- block's is what makes `(drawnIndex, drawnRate, t)` a self-consistent
    -- interest checkpoint (§5.3). The mirror reads it from here.
    block_timestamp DateTime('UTC'),
    tx_hash         String,
    tx_index        UInt32,
    event_name      LowCardinality(String),
    -- The indexed parameters, in ABI order. topic0 is the signature hash, which
    -- event_name already says.
    --
    -- topic1 is the asset id on all thirteen. topic2 is not one thing: the
    -- drawing spoke on Add/Remove/Draw/Restore/RefreshPremium/ReportDeficit,
    -- the *calling* spoke on EliminateDeficit, the fee receiver on
    -- MintFeeShares, the reinvestment controller on Sweep/Reclaim, the
    -- underlying token on AddAsset, and null on UpdateAsset and
    -- UpdateAssetConfig, which index only the asset. topic3 is set on
    -- EliminateDeficit alone, where it is the covered spoke — the one whose
    -- shares are burned, and so the one the deficit belongs to.
    topic1          Nullable(String),
    topic2          Nullable(String),
    topic3          Nullable(String),
    -- Every decoded argument, indexed and not, each uint256 already a string:
    -- narrowing one to a number because it looks small is the habit §7.5 is
    -- about. Here it would be fatal rather than merely wrong — `drawnIndex` is
    -- RAY-scaled and starts above 1e27.
    body            String,
    -- The raw non-indexed bytes, kept beside the decoded body so a decoder bug
    -- is repairable by re-decoding what is already stored instead of
    -- re-fetching the backfill.
    data            String,
    -- +1 live, -1 retraction.
    sign            Int8
)
ENGINE = VersionedCollapsingMergeTree(sign, version)
PARTITION BY intDiv(block_number, 1000000)
ORDER BY (chain_id, block_number, log_index)
