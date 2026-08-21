//! Every `.sql` file this deployment applies, embedded at compile time.
//!
//! **Two lists, never one.** The ordinals are unique within a database and not
//! across them — `001_spoke_events` and `001_indexer_cursor` both exist — so
//! concatenating these would collide, and then try to apply Postgres DDL to
//! ClickHouse.
//!
//! Grouped by the directory each came from, rather than flattened, because a
//! group is what a crate would own if the schema ever followed its code. It
//! does not: `bins/migrate` embeds this to apply it and `aave-positions` reads
//! it to build a scratch database, and putting a group inside the crate that
//! uses it would make the migrator link that crate — the read store, alloy and
//! all — to reach a table of string literals. The directory travels with its
//! list so the completeness test can hold the two to each other.
//!
//! **A corpus, not a runner.** Nothing here knows what applying a migration
//! means, which is why this crate has no dependencies. `label` is the one
//! concession: refinery's parser wants `V{version}__{name}`, and spelling it
//! here rather than deriving it there keeps the ledger's keys a thing you can
//! read rather than compute.
//!
//! The paths reach into `packages/` because the TypeScript runner reads the
//! same files. They come in here when `packages/` goes in Phase 5.

/// One `.sql` file, and the two names it goes by.
pub struct Embedded {
    /// The basename without `.sql`, which is what the directory calls it. Only
    /// the completeness test reads this.
    pub file: &'static str,
    /// What refinery calls it. Its parser requires `V{version}__{name}`, and
    /// the version has to be an integer, so `012_position_supply` becomes
    /// `V12__position_supply` — same order, refinery's spelling.
    pub label: &'static str,
    /// The whole file, sent as it stands. One statement per file, so nothing
    /// here has to be taken apart before it reaches a server.
    pub sql: &'static str,
}

/// One future crate's worth of schema, and where it is read from.
pub struct Source {
    /// Relative to this crate's manifest, which is what a test's cwd is.
    pub directory: &'static str,
    pub files: &'static [Embedded],
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
                file: "012_position_supply",
                label: "V12__position_supply",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/012_position_supply.sql"
                ),
            },
            Embedded {
                file: "013_position_withdraw",
                label: "V13__position_withdraw",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/013_position_withdraw.sql"
                ),
            },
            Embedded {
                file: "014_position_borrow",
                label: "V14__position_borrow",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/014_position_borrow.sql"
                ),
            },
            Embedded {
                file: "015_position_repay",
                label: "V15__position_repay",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/015_position_repay.sql"
                ),
            },
            Embedded {
                file: "016_position_report_deficit",
                label: "V16__position_report_deficit",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/016_position_report_deficit.sql"
                ),
            },
            Embedded {
                file: "017_position_liquidation_collateral",
                label: "V17__position_liquidation_collateral",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/017_position_liquidation_collateral.sql"
                ),
            },
            Embedded {
                file: "018_position_liquidation_debt",
                label: "V18__position_liquidation_debt",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/018_position_liquidation_debt.sql"
                ),
            },
            Embedded {
                file: "019_position_liquidation_liquidator",
                label: "V19__position_liquidation_liquidator",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/019_position_liquidation_liquidator.sql"
                ),
            },
            Embedded {
                file: "020_position_collateral_flag",
                label: "V20__position_collateral_flag",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/020_position_collateral_flag.sql"
                ),
            },
            Embedded {
                file: "021_user_positions_current",
                label: "V21__user_positions_current",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/021_user_positions_current.sql"
                ),
            },
            Embedded {
                file: "030_hub_assets",
                label: "V30__hub_assets",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/030_hub_assets.sql"
                ),
            },
            Embedded {
                file: "031_hub_asset_state",
                label: "V31__hub_asset_state",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/031_hub_asset_state.sql"
                ),
            },
            Embedded {
                file: "032_hub_add",
                label: "V32__hub_add",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/032_hub_add.sql"
                ),
            },
            Embedded {
                file: "033_hub_remove",
                label: "V33__hub_remove",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/033_hub_remove.sql"
                ),
            },
            Embedded {
                file: "034_hub_draw",
                label: "V34__hub_draw",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/034_hub_draw.sql"
                ),
            },
            Embedded {
                file: "035_hub_restore",
                label: "V35__hub_restore",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/035_hub_restore.sql"
                ),
            },
            Embedded {
                file: "036_hub_report_deficit",
                label: "V36__hub_report_deficit",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/036_hub_report_deficit.sql"
                ),
            },
            Embedded {
                file: "037_hub_eliminate_deficit",
                label: "V37__hub_eliminate_deficit",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/037_hub_eliminate_deficit.sql"
                ),
            },
            Embedded {
                file: "038_hub_mint_fee_shares",
                label: "V38__hub_mint_fee_shares",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/038_hub_mint_fee_shares.sql"
                ),
            },
            Embedded {
                file: "039_hub_sweep",
                label: "V39__hub_sweep",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/039_hub_sweep.sql"
                ),
            },
            Embedded {
                file: "040_hub_reclaim",
                label: "V40__hub_reclaim",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/040_hub_reclaim.sql"
                ),
            },
            Embedded {
                file: "041_hub_refresh_premium",
                label: "V41__hub_refresh_premium",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/041_hub_refresh_premium.sql"
                ),
            },
            Embedded {
                file: "042_hub_update_asset",
                label: "V42__hub_update_asset",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/042_hub_update_asset.sql"
                ),
            },
            Embedded {
                file: "043_hub_update_asset_config",
                label: "V43__hub_update_asset_config",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/043_hub_update_asset_config.sql"
                ),
            },
            Embedded {
                file: "044_hub_add_asset",
                label: "V44__hub_add_asset",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/044_hub_add_asset.sql"
                ),
            },
            Embedded {
                file: "045_hub_assets_current",
                label: "V45__hub_assets_current",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/045_hub_assets_current.sql"
                ),
            },
            Embedded {
                file: "050_spoke_reserves",
                label: "V50__spoke_reserves",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/050_spoke_reserves.sql"
                ),
            },
            Embedded {
                file: "051_spoke_reserve_registry",
                label: "V51__spoke_reserve_registry",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/051_spoke_reserve_registry.sql"
                ),
            },
            Embedded {
                file: "052_spoke_reserves_current",
                label: "V52__spoke_reserves_current",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/052_spoke_reserves_current.sql"
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::{CLICKHOUSE, Embedded, POSTGRES, Source};

    /// Rejects a group that has drifted from the directory it was embedded from.
    ///
    /// The lists above are written by hand, so nothing keeps them in step with the directory
    /// on its own — and refinery cannot help, because it only ever sees the list it
    /// is handed. Neither half can move without the other: a `.sql` file nobody added
    /// fails, and a list entry whose file was deleted or renamed fails too.
    ///
    /// Compares file *names*, not refinery's labels: `001_spoke_events.sql` is on
    /// disk, `V1__spoke_events` is what refinery calls it, and it is the first of
    /// those the directory can be asked about.
    ///
    /// # Errors
    ///
    /// [`Error::Unreadable`] if the directory cannot be listed — which matters, since
    /// a wrong path returning `Ok` would make the whole guarantee vacuous — and
    /// [`Error::NotEmbedded`] or [`Error::NotOnDisk`] for a disagreement either way.
    fn check_complete(files: &[Embedded], directory: &Path) -> Result<(), Error> {
        let unreadable = |source| Error::Unreadable {
            directory: directory.to_path_buf(),
            source,
        };

        let mut names: Vec<String> = Vec::new();
        for entry in fs::read_dir(directory).map_err(unreadable)? {
            let name = entry.map_err(unreadable)?.file_name();
            // A name that is not UTF-8 cannot be one of ours: the entries are Rust
            // string literals. Skipping it is the same answer as not matching `.sql`.
            let Some(name) = name.to_str() else { continue };

            if let Some(id) = name.strip_suffix(".sql") {
                names.push(id.to_owned());
            }
        }

        // `names` owns the strings `on_disk` borrows.
        let on_disk: BTreeSet<&str> = names.iter().map(String::as_str).collect();
        let embedded: BTreeSet<&str> = files.iter().map(|file| file.file).collect();

        if let Some(&id) = on_disk.difference(&embedded).next() {
            return Err(Error::NotEmbedded {
                directory: directory.to_path_buf(),
                id: id.to_owned(),
            });
        }
        if let Some(&id) = embedded.difference(&on_disk).next() {
            return Err(Error::NotOnDisk {
                directory: directory.to_path_buf(),
                id: id.to_owned(),
            });
        }

        Ok(())
    }

    #[derive(Debug, thiserror::Error)]
    enum Error {
        #[error("{}/{id}.sql is not embedded by any entry here", directory.display())]
        NotEmbedded { directory: PathBuf, id: String },

        #[error("\"{id}\" is embedded here and is not a .sql file in {}", directory.display())]
        NotOnDisk { directory: PathBuf, id: String },

        #[error("could not read {}", directory.display())]
        Unreadable {
            directory: PathBuf,
            #[source]
            source: std::io::Error,
        },
    }

    /// A miniature of a group above, pointed at this crate's fixtures.
    const FIXTURES: &[Embedded] = &[
        Embedded {
            file: "001_opens_with_prose",
            label: "V1__opens_with_prose",
            sql: include_str!("../fixtures/001_opens_with_prose.sql"),
        },
        Embedded {
            file: "002_second_file",
            label: "V2__second_file",
            sql: include_str!("../fixtures/002_second_file.sql"),
        },
    ];

    #[test]
    fn accepts_a_group_that_matches_its_directory() {
        check_complete(FIXTURES, Path::new("fixtures")).unwrap();
    }

    #[test]
    fn rejects_a_file_nobody_added_to_schema() {
        let missed = check_complete(&FIXTURES[..1], Path::new("fixtures")).unwrap_err();

        assert!(
            missed
                .to_string()
                .ends_with("/002_second_file.sql is not embedded by any entry here"),
            "{missed}"
        );
    }

    #[test]
    fn rejects_an_entry_whose_file_is_gone() {
        // Every file on disk stays listed, so the only disagreement is the entry
        // pointing at a file that is not there.
        let renamed = [
            Embedded {
                file: "001_opens_with_prose",
                label: "V1__a",
                sql: "",
            },
            Embedded {
                file: "002_second_file",
                label: "V2__b",
                sql: "",
            },
            Embedded {
                file: "003_deleted",
                label: "V3__deleted",
                sql: "",
            },
        ];

        let gone = check_complete(&renamed, Path::new("fixtures")).unwrap_err();

        assert!(
            gone.to_string()
                .starts_with("\"003_deleted\" is embedded here and is not a .sql file in "),
            "{gone}"
        );
    }

    #[test]
    fn says_which_directory_it_could_not_read() {
        // A wrong path answering Ok would make every other completeness test in
        // the workspace vacuous.
        let absent = Path::new("fixtures/no_such_directory");

        let error = check_complete(FIXTURES, absent).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("could not read {}", absent.display())
        );
    }

    /// The guard as the deployment actually needs it: every group in the corpus
    /// against the directory it was embedded from. Per group, because each group
    /// is exactly one directory.
    #[test]
    fn every_group_matches_the_directory_it_was_embedded_from() {
        for Source { directory, files } in CLICKHOUSE.iter().chain(POSTGRES) {
            check_complete(files, Path::new(directory))
                .unwrap_or_else(|error| panic!("{directory}: {error}"));
        }
    }
}
