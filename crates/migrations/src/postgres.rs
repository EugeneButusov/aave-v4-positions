//! The cursor, the reorg window, and the two enrichment dimensions.
//!
//! Its own ordinal space, which is why it is its own list — see the crate doc.

use crate::{Embedded, Source};

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
