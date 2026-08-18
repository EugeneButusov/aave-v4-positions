//! Every `.sql` file this deployment applies, embedded at compile time.
//!
//! **Two lists, never one.** The ordinals are unique within a database and not
//! across them — `001_spoke_events` and `001_indexer_cursor` both exist — so
//! concatenating these would collide, and then try to apply Postgres DDL to
//! ClickHouse.
//!
//! Grouped by the directory each came from, rather than flattened, because a
//! group is what a crate will own once Phase 3 and 4 create them: `events` takes
//! the first block, `positions` the second, and this file shrinks each time. The
//! directory travels with its list so a test can hold the two to each other.
//!
//! The paths reach into `packages/` because the TypeScript runner reads the same
//! files. They come in here when `packages/` goes in Phase 5.

use refinery_core::{Error, Migration};

/// One `.sql` file, and the two names it goes by.
pub struct Embedded {
    /// The basename without `.sql`, which is what the directory calls it. Only
    /// the completeness test reads this.
    #[cfg_attr(not(test), allow(dead_code))]
    pub file: &'static str,
    /// What refinery calls it. Its parser requires `V{version}__{name}`, and
    /// the version has to be an integer, so `012_position_projections` becomes
    /// `V12__position_projections` — same order, refinery's spelling.
    pub label: &'static str,
    pub sql: &'static str,
}

/// One future crate's worth of schema, and where it is read from.
pub struct Source {
    /// Relative to this crate's manifest, which is what a test's cwd is.
    #[cfg_attr(not(test), allow(dead_code))]
    pub directory: &'static str,
    pub files: &'static [Embedded],
}

/// Flattens the groups into the migration set refinery is handed.
///
/// # Errors
///
/// Propagates refinery's parse error if a label is not `V{version}__{name}` —
/// which would be a typo in this file, caught the first time it runs.
pub fn union(sources: &[Source]) -> Result<Vec<Migration>, Error> {
    sources
        .iter()
        .flat_map(|source| source.files)
        .map(|embedded| Migration::unapplied(embedded.label, embedded.sql))
        .collect()
}

/// The ClickHouse schema: the two append-only ledgers and the folds over them.
pub const CLICKHOUSE: &[Source] = &[
    // the Spoke and Hub event ledgers
    Source {
        directory: "../../packages/aave-positions/events/src/store/clickhouse-migrations",
        files: &[
            Embedded {
                file: "001_spoke_events",
                label: "V1__spoke_events",
                sql: include_str!(
                    "../../../packages/aave-positions/events/src/store/clickhouse-migrations/001_spoke_events.sql"
                ),
            },
            Embedded {
                file: "002_spoke_events_current",
                label: "V2__spoke_events_current",
                sql: include_str!(
                    "../../../packages/aave-positions/events/src/store/clickhouse-migrations/002_spoke_events_current.sql"
                ),
            },
            Embedded {
                file: "003_hub_events",
                label: "V3__hub_events",
                sql: include_str!(
                    "../../../packages/aave-positions/events/src/store/clickhouse-migrations/003_hub_events.sql"
                ),
            },
            Embedded {
                file: "004_hub_events_current",
                label: "V4__hub_events_current",
                sql: include_str!(
                    "../../../packages/aave-positions/events/src/store/clickhouse-migrations/004_hub_events_current.sql"
                ),
            },
        ],
    },
    // the folds over them
    Source {
        directory: "../../packages/aave-positions/positions/src/store/clickhouse-migrations",
        files: &[
            Embedded {
                file: "010_user_positions",
                label: "V10__user_positions",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/010_user_positions.sql"
                ),
            },
            Embedded {
                file: "011_user_position_flags",
                label: "V11__user_position_flags",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/011_user_position_flags.sql"
                ),
            },
            Embedded {
                file: "012_position_projections",
                label: "V12__position_projections",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/012_position_projections.sql"
                ),
            },
            Embedded {
                file: "013_user_positions_current",
                label: "V13__user_positions_current",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/013_user_positions_current.sql"
                ),
            },
            Embedded {
                file: "020_hub_assets",
                label: "V20__hub_assets",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/020_hub_assets.sql"
                ),
            },
            Embedded {
                file: "021_hub_asset_state",
                label: "V21__hub_asset_state",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/021_hub_asset_state.sql"
                ),
            },
            Embedded {
                file: "022_hub_asset_projections",
                label: "V22__hub_asset_projections",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/022_hub_asset_projections.sql"
                ),
            },
            Embedded {
                file: "023_hub_assets_current",
                label: "V23__hub_assets_current",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/023_hub_assets_current.sql"
                ),
            },
            Embedded {
                file: "030_spoke_reserves",
                label: "V30__spoke_reserves",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/030_spoke_reserves.sql"
                ),
            },
            Embedded {
                file: "031_spoke_reserve_registry",
                label: "V31__spoke_reserve_registry",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/031_spoke_reserve_registry.sql"
                ),
            },
        ],
    },
];

/// The Postgres schema: the indexer's own position, and the two enrichment
/// dimensions.
pub const POSTGRES: &[Source] = &[
    // the indexer's own position
    Source {
        directory: "../../packages/indexing/src/postgres-migrations",
        files: &[
            Embedded {
                file: "001_indexer_cursor",
                label: "V1__indexer_cursor",
                sql: include_str!(
                    "../../../packages/indexing/src/postgres-migrations/001_indexer_cursor.sql"
                ),
            },
            Embedded {
                file: "002_indexer_block_headers",
                label: "V2__indexer_block_headers",
                sql: include_str!(
                    "../../../packages/indexing/src/postgres-migrations/002_indexer_block_headers.sql"
                ),
            },
        ],
    },
    // ERC-20 symbol and name
    Source {
        directory: "../../packages/token-metadata/src/migrations",
        files: &[Embedded {
            file: "010_token_metadata",
            label: "V10__token_metadata",
            sql: include_str!(
                "../../../packages/token-metadata/src/migrations/010_token_metadata.sql"
            ),
        }],
    },
    // the Spoke oracle
    Source {
        directory: "../../packages/prices/src/migrations",
        files: &[Embedded {
            file: "011_reserve_prices",
            label: "V11__reserve_prices",
            sql: include_str!("../../../packages/prices/src/migrations/011_reserve_prices.sql"),
        }],
    },
];
