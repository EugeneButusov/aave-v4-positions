//! What an ERC-20 calls itself.
//!
//! Enrichment, not fold: no Aave event carries a symbol (§12.5), so this is
//! read from the token contract and kept beside the event log. Why it lives in
//! Postgres is measured in `010_token_metadata.sql`.
//!
//! The read half only — `put` and the sweep that calls it are Phase 3's.
//!
//! The port is this level; the adapter and its harness are a directory down.

mod error;
mod label;
mod postgres;
mod token_metadata_store;

#[cfg(test)]
mod conformance;

pub use error::Error;
pub use label::TokenLabel;
pub use postgres::PostgresTokenMetadataStore;
pub use token_metadata_store::TokenMetadataStore;
