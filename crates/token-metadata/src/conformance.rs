//! The [`TokenMetadataStore`] port as an executable specification.
//!
//! Every implementation runs it. What the port cannot supply is the rows
//! themselves — it is read-only until the enrichment processor arrives — so an
//! implementation also supplies a [`Fixture`] saying how state gets in front of
//! it.

use std::collections::HashMap;

use alloy_primitives::{Address, address};

use crate::{TokenLabel, TokenMetadataStore};

pub(crate) const CHAIN_ID: u32 = 1;
pub(crate) const OTHER_CHAIN: u32 = 8453;

/// Stored lower-cased, which the table's own `CHECK` enforces.
pub(crate) const USDC: Address = address!("a0b86991c6218b36c1d19d4a2e9eb0ce3606eb48");
pub(crate) const WETH: Address = address!("c02aaa39b223fe8d0a0e5c4f27ead9083c756cc2");
/// Listed by the Hub, never enriched — the token that has no row at all.
pub(crate) const UNASKED: Address = address!("6b175474e89094c44da98b954eedeac495271d0f");

/// One row of `token_metadata`, as a case wants to describe it.
pub(crate) struct Stored {
    pub chain_id: u32,
    pub token: Address,
    pub symbol: Option<&'static str>,
    pub name: Option<&'static str>,
}

impl Stored {
    pub(crate) fn labelled(token: Address, symbol: &'static str) -> Self {
        Self {
            chain_id: CHAIN_ID,
            token,
            symbol: Some(symbol),
            name: Some(symbol),
        }
    }

    /// A token that answered, and had nothing to say. Conformant under EIP-20,
    /// where both fields are OPTIONAL.
    pub(crate) fn mute(token: Address) -> Self {
        Self {
            chain_id: CHAIN_ID,
            token,
            symbol: None,
            name: None,
        }
    }

    pub(crate) fn on(chain_id: u32, token: Address) -> Self {
        Self {
            chain_id,
            ..Self::labelled(token, "ELSEWHERE")
        }
    }
}

pub(crate) trait Fixture {
    type Store: TokenMetadataStore;

    /// A database of its own, named for the case, so cases can run at once.
    async fn fresh(case: &str) -> Self;

    fn store(&self) -> &Self::Store;

    async fn given_labels(&self, rows: &[Stored]);
}

async fn labels_of<F: Fixture>(fixture: &F) -> HashMap<Address, TokenLabel> {
    fixture
        .store()
        .labels(CHAIN_ID)
        .await
        .expect("the dimension should read")
}

pub(crate) async fn reads_every_label_on_the_chain<F: Fixture>() {
    let fixture = F::fresh("reads_every_label_on_the_chain").await;
    fixture
        .given_labels(&[
            Stored::labelled(USDC, "USDC"),
            Stored::labelled(WETH, "WETH"),
        ])
        .await;

    let labels = labels_of(&fixture).await;

    assert_eq!(labels.len(), 2);
    assert_eq!(labels[&USDC].symbol.as_deref(), Some("USDC"));
    assert_eq!(labels[&WETH].symbol.as_deref(), Some("WETH"));
}

pub(crate) async fn leaves_out_a_chain_it_was_not_asked_about<F: Fixture>() {
    // The dimension is per chain, and the same token is listed on several.
    let fixture = F::fresh("leaves_out_a_chain_it_was_not_asked_about").await;
    fixture
        .given_labels(&[
            Stored::labelled(USDC, "USDC"),
            Stored::on(OTHER_CHAIN, WETH),
        ])
        .await;

    let labels = labels_of(&fixture).await;

    assert_eq!(labels.len(), 1, "{labels:?}");
    assert!(labels.contains_key(&USDC));
}

pub(crate) async fn keeps_a_token_that_answered_with_no_symbol<F: Fixture>() {
    // The distinction the nullable columns exist for: this token was asked and
    // had nothing to say, which is not the same as never having been asked.
    let fixture = F::fresh("keeps_a_token_that_answered_with_no_symbol").await;
    fixture.given_labels(&[Stored::mute(USDC)]).await;

    let labels = labels_of(&fixture).await;

    assert!(labels.contains_key(&USDC), "the row records the question");
    assert_eq!(
        labels[&USDC],
        TokenLabel {
            symbol: None,
            name: None
        }
    );
}

pub(crate) async fn omits_a_token_with_no_row<F: Fixture>() {
    // The other half of that distinction, and the reason it is presence rather
    // than a null: this one is a gap for the sweep to fill.
    let fixture = F::fresh("omits_a_token_with_no_row").await;
    fixture
        .given_labels(&[Stored::labelled(USDC, "USDC")])
        .await;

    let labels = labels_of(&fixture).await;

    assert!(!labels.contains_key(&UNASKED));
}

pub(crate) async fn answers_a_checksummed_address_from_a_lower_cased_row<F: Fixture>() {
    // What the TypeScript needs `reserveKey`-style lower-casing discipline for.
    // The column is lower-cased — its `CHECK` allows nothing else — and this
    // key was parsed from the checksummed spelling a caller would hold. One
    // `Address`, one hash, no rule to forget.
    let fixture = F::fresh("answers_a_checksummed_address_from_a_lower_cased_row").await;
    fixture
        .given_labels(&[Stored::labelled(USDC, "USDC")])
        .await;

    let checksummed: Address = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48"
        .parse()
        .expect("a checksummed address parses");

    assert_eq!(
        labels_of(&fixture).await[&checksummed].symbol.as_deref(),
        Some("USDC")
    );
}

pub(crate) async fn is_empty_for_a_chain_nobody_has_enriched<F: Fixture>() {
    let fixture = F::fresh("is_empty_for_a_chain_nobody_has_enriched").await;

    assert!(labels_of(&fixture).await.is_empty());
}
