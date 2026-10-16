//! What a page looks like on the wire: the API's own view of a folded position.
//!
//! A view, not the domain: `aave_positions::store` holds what a position is,
//! these are what a caller is shown. [`page`] wraps the positions and their
//! clocks, [`item`](mod@item) is one position.
//!
//! The rules all six types follow: no `rename_all` anywhere, every multi-word
//! key snake_case; they mirror the domain rather than reuse it, because the
//! domain may gain a field and the contract may not; every number a decimal
//! string scaled by `aave_positions::scale`; every instant RFC 3339 in UTC;
//! null means unknown, never zero, since §7.4's oracle reverts rather than
//! answer one. Field order is declaration order.

mod item;
mod page;

use std::collections::HashMap;

use aave_positions::store::Position;
use aave_positions::valuation;
use alloy_primitives::Address;
use prices::{ReserveKey, ReservePrice};
use token_metadata::TokenLabel;

pub(crate) use item::Item;
pub(crate) use page::Page;

/// A page this port could not render.
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Value(#[from] valuation::Error),

    #[error("{0} is not an instant a date can hold")]
    ValuedAt(u64),
}

/// Prices, keyed as the store hands them back.
pub(crate) type Prices = HashMap<ReserveKey, ReservePrice>;

/// Token labels, keyed as the enrichment hands them back.
pub(crate) type Labels = HashMap<Address, TokenLabel>;

/// The price this position would be valued with, if there is one.
///
/// Gated on `value` as well as `asset`: an unused price must not drag the
/// page's clock backwards.
fn price_for<'a>(position: &Position, prices: &'a Prices) -> Option<&'a ReservePrice> {
    position.asset.as_ref()?;
    position.value.as_ref()?;

    prices.get(&ReserveKey {
        spoke: position.spoke,
        reserve_id: position.reserve_id,
    })
}
