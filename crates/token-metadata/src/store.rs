//! The port: what a caller asks for, and what it gets back.

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::{Error, TokenLabel};

/// Reads what tokens call themselves.
///
/// **No pagination.** A Hub lists seventeen assets on mainnet, the whole
/// dimension fits in one response, and the read path wants all of it at once.
#[async_trait::async_trait]
pub trait TokenMetadataStore: Send + Sync {
    /// Every label on this chain, keyed by token.
    ///
    /// **The whole dimension, deliberately not the tokens on a page.** Taking a
    /// page's addresses would make this depend on the page and force it to run
    /// after the ClickHouse query; keyed only by chain it runs *beside* it,
    /// which is what makes the second round trip free — measured at 0.27 ms
    /// against a 28 ms page.
    ///
    /// A token with no row is **absent from the map**, not present with nulls:
    /// the caller has to tell "never asked" from "asked, and it has no symbol",
    /// because one is a gap to fill and the other is not.
    ///
    /// **Keyed by [`Address`] rather than by a string.** The TypeScript keys
    /// this map on a lower-cased hex string and its store doc warns that the
    /// moment either end spells the key itself, one of them forgets to
    /// lower-case and every label silently stops joining. An `Address` has one
    /// representation and hashes by its twenty bytes, so a checksummed address
    /// and a lower-cased one are the same key and there is no rule left to
    /// forget.
    ///
    /// # Errors
    ///
    /// [`Error`] when the query cannot be taken or a row cannot be read.
    async fn labels(&self, chain_id: u32) -> Result<HashMap<Address, TokenLabel>, Error>;
}
