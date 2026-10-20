//! The Postgres implementation of [`TokenMetadataStore`](crate::TokenMetadataStore).
//!
//! Every case here is the port's, in `conformance`, run through `Harness`.

#[cfg(test)]
mod harness;
mod token_metadata_store;

pub use token_metadata_store::PostgresTokenMetadataStore;
