//! The Postgres implementation of [`TokenMetadataStore`](crate::TokenMetadataStore).
//!
//! `token_metadata_store` is the statement, the row it comes back as and the
//! read that joins them. `harness` stands up the migrated schema its specs read
//! from.
//!
//! The specs split the same way. Every case here is the port's, in
//! `conformance`, run through this adapter's `Harness`; nothing about this
//! implementation needs one of its own yet.

#[cfg(test)]
mod harness;
mod token_metadata_store;

pub use token_metadata_store::PostgresTokenMetadataStore;
