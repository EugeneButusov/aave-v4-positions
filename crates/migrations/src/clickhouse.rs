//! The event ledgers and the folds over them.
//!
//! Thirty-five files, and the ordinals run across the directories rather than
//! within them: `010` beats `002` wherever each lives, which is what puts a
//! projection after the table it reads.

use crate::{Embedded, Source};

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
            Embedded {
                file: "060_drop_position_supply",
                label: "V60__drop_position_supply",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/060_drop_position_supply.sql"
                ),
            },
            Embedded {
                file: "061_drop_position_withdraw",
                label: "V61__drop_position_withdraw",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/061_drop_position_withdraw.sql"
                ),
            },
            Embedded {
                file: "062_drop_position_borrow",
                label: "V62__drop_position_borrow",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/062_drop_position_borrow.sql"
                ),
            },
            Embedded {
                file: "063_drop_position_repay",
                label: "V63__drop_position_repay",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/063_drop_position_repay.sql"
                ),
            },
            Embedded {
                file: "064_drop_position_report_deficit",
                label: "V64__drop_position_report_deficit",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/064_drop_position_report_deficit.sql"
                ),
            },
            Embedded {
                file: "065_drop_position_liquidation_collateral",
                label: "V65__drop_position_liquidation_collateral",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/065_drop_position_liquidation_collateral.sql"
                ),
            },
            Embedded {
                file: "066_drop_position_liquidation_debt",
                label: "V66__drop_position_liquidation_debt",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/066_drop_position_liquidation_debt.sql"
                ),
            },
            Embedded {
                file: "067_drop_position_liquidation_liquidator",
                label: "V67__drop_position_liquidation_liquidator",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/067_drop_position_liquidation_liquidator.sql"
                ),
            },
            Embedded {
                file: "068_drop_user_positions",
                label: "V68__drop_user_positions",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/068_drop_user_positions.sql"
                ),
            },
            Embedded {
                file: "069_user_positions",
                label: "V69__user_positions",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/069_user_positions.sql"
                ),
            },
            Embedded {
                file: "070_position_supply",
                label: "V70__position_supply",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/070_position_supply.sql"
                ),
            },
            Embedded {
                file: "071_position_withdraw",
                label: "V71__position_withdraw",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/071_position_withdraw.sql"
                ),
            },
            Embedded {
                file: "072_position_borrow",
                label: "V72__position_borrow",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/072_position_borrow.sql"
                ),
            },
            Embedded {
                file: "073_position_repay",
                label: "V73__position_repay",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/073_position_repay.sql"
                ),
            },
            Embedded {
                file: "074_position_report_deficit",
                label: "V74__position_report_deficit",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/074_position_report_deficit.sql"
                ),
            },
            Embedded {
                file: "075_position_liquidation_collateral",
                label: "V75__position_liquidation_collateral",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/075_position_liquidation_collateral.sql"
                ),
            },
            Embedded {
                file: "076_position_liquidation_debt",
                label: "V76__position_liquidation_debt",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/076_position_liquidation_debt.sql"
                ),
            },
            Embedded {
                file: "077_position_liquidation_liquidator",
                label: "V77__position_liquidation_liquidator",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/077_position_liquidation_liquidator.sql"
                ),
            },
            Embedded {
                file: "078_fill_position_supply",
                label: "V78__fill_position_supply",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/078_fill_position_supply.sql"
                ),
            },
            Embedded {
                file: "079_fill_position_withdraw",
                label: "V79__fill_position_withdraw",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/079_fill_position_withdraw.sql"
                ),
            },
            Embedded {
                file: "080_fill_position_borrow",
                label: "V80__fill_position_borrow",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/080_fill_position_borrow.sql"
                ),
            },
            Embedded {
                file: "081_fill_position_repay",
                label: "V81__fill_position_repay",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/081_fill_position_repay.sql"
                ),
            },
            Embedded {
                file: "082_fill_position_report_deficit",
                label: "V82__fill_position_report_deficit",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/082_fill_position_report_deficit.sql"
                ),
            },
            Embedded {
                file: "083_fill_position_liquidation_collateral",
                label: "V83__fill_position_liquidation_collateral",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/083_fill_position_liquidation_collateral.sql"
                ),
            },
            Embedded {
                file: "084_fill_position_liquidation_debt",
                label: "V84__fill_position_liquidation_debt",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/084_fill_position_liquidation_debt.sql"
                ),
            },
            Embedded {
                file: "085_fill_position_liquidation_liquidator",
                label: "V85__fill_position_liquidation_liquidator",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/085_fill_position_liquidation_liquidator.sql"
                ),
            },
            Embedded {
                file: "086_user_position_flags_timestamp",
                label: "V86__user_position_flags_timestamp",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/086_user_position_flags_timestamp.sql"
                ),
            },
            Embedded {
                file: "087_position_collateral_flag",
                label: "V87__position_collateral_flag",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/087_position_collateral_flag.sql"
                ),
            },
            Embedded {
                file: "088_truncate_user_position_flags",
                label: "V88__truncate_user_position_flags",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/088_truncate_user_position_flags.sql"
                ),
            },
            Embedded {
                file: "089_fill_position_collateral_flag",
                label: "V89__fill_position_collateral_flag",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/089_fill_position_collateral_flag.sql"
                ),
            },
            Embedded {
                file: "090_user_positions_as_of",
                label: "V90__user_positions_as_of",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/090_user_positions_as_of.sql"
                ),
            },
            Embedded {
                file: "091_drop_user_positions_current",
                label: "V91__drop_user_positions_current",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/091_drop_user_positions_current.sql"
                ),
            },
            Embedded {
                file: "092_spoke_reserves_timestamp",
                label: "V92__spoke_reserves_timestamp",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/092_spoke_reserves_timestamp.sql"
                ),
            },
            Embedded {
                file: "093_spoke_reserve_registry",
                label: "V93__spoke_reserve_registry",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/093_spoke_reserve_registry.sql"
                ),
            },
            Embedded {
                file: "094_truncate_spoke_reserves",
                label: "V94__truncate_spoke_reserves",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/094_truncate_spoke_reserves.sql"
                ),
            },
            Embedded {
                file: "095_fill_spoke_reserve_registry",
                label: "V95__fill_spoke_reserve_registry",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/095_fill_spoke_reserve_registry.sql"
                ),
            },
            Embedded {
                file: "096_spoke_reserves_as_of",
                label: "V96__spoke_reserves_as_of",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/096_spoke_reserves_as_of.sql"
                ),
            },
            Embedded {
                file: "097_drop_spoke_reserves_current",
                label: "V97__drop_spoke_reserves_current",
                sql: include_str!(
                    "../../../packages/aave-positions/positions/src/store/clickhouse-migrations/097_drop_spoke_reserves_current.sql"
                ),
            },
        ],
    },
];
