//! What an ERC-20 calls itself.
//!
//! **Enrichment, not fold.** No Aave event carries a symbol (§12.5), so this is
//! read from the token contract and kept beside the event log rather than
//! derived from it. That is why it lives in Postgres and why the row is written
//! rather than projected — `010_token_metadata.sql` has the measurements.
//!
//! **The read half only.** `put` and the sweep that calls it belong to the
//! enrichment processor, which is Phase 3's; nothing in this workspace writes a
//! label yet, and a method with no caller is a surface with no test behind it.
//! The trait gains it when the processor arrives.

mod error;
mod label;
mod postgres;
mod store;

#[cfg(test)]
mod conformance;
#[cfg(test)]
mod harness;

pub use error::Error;
pub use label::TokenLabel;
pub use postgres::PostgresTokenMetadataStore;
pub use store::TokenMetadataStore;
