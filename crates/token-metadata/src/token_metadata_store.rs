//! The port: what a caller asks for, and what it gets back.

use std::collections::HashMap;

use alloy_primitives::Address;

use units::ChainId;

use crate::{Error, TokenLabel};

/// Reads what tokens call themselves.
///
/// No pagination: a Hub lists seventeen assets on mainnet.
#[async_trait::async_trait]
pub trait TokenMetadataStore: Send + Sync {
    /// Every label on this chain, keyed by token.
    ///
    /// The whole dimension, not the tokens on a page: taking a page's
    /// addresses would force this to run after the ClickHouse query, where
    /// keyed by chain alone it runs beside it — 0.27 ms against a 28 ms page.
    ///
    /// A token with no row is absent from the map rather than present with
    /// nulls, so a caller can tell "never asked" from "asked, and has none".
    ///
    /// Keyed by [`Address`] rather than a string, which has one representation
    /// and hashes by its twenty bytes — so there is no spelling to get wrong.
    ///
    /// # Errors
    ///
    /// [`Error`] when the query cannot be taken or a row cannot be read.
    async fn labels(&self, chain_id: ChainId) -> Result<HashMap<Address, TokenLabel>, Error>;
}
