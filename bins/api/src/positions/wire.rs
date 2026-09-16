//! What a page looks like on the wire, and how a folded position becomes one.
//!
//! [`page`] is what wraps the positions and the two clocks beside them, and
//! [`item`](mod@item) is one position. Each holds the shapes it owns and the
//! conversion that builds them.
//!
//! Four rules govern all six types. **No `rename_all` anywhere** — every
//! multi-word key is snake_case. **They mirror the domain rather than reuse
//! it**, which is why the wire item is `Item` and not `Position`: the domain may
//! gain a field, the contract may not. **Every number is a decimal string**,
//! scaled by `aave_positions::scale`. **Every instant is RFC 3339 in UTC**, as
//! `time::serde::rfc3339` writes it. **Null means unknown, never zero** —
//! §7.4's oracle reverts rather than answer one, so a zero could not be told
//! from a real one. Field order is declaration order.

mod item;
mod page;

use std::collections::HashMap;

use aave_positions::store::Position;
use aave_positions::valuation;
use prices::{ReserveKey, ReservePrice};

pub(crate) use item::Item;
pub(crate) use page::{Page, Progress, pricing};

/// A folded position this port could not render.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub(crate) struct Error(#[from] valuation::Error);

/// Prices, keyed as the store hands them back.
pub(crate) type Prices = HashMap<ReserveKey, ReservePrice>;

/// The price this position would be valued with, if there is one.
///
/// Gated on `value` as well as `asset`: with nothing to price, a price is not
/// "used" and must not drag the page's clock backwards.
fn price_for<'a>(position: &Position, prices: &'a Prices) -> Option<&'a ReservePrice> {
    position.asset.as_ref()?;
    position.value.as_ref()?;

    prices.get(&ReserveKey {
        spoke: position.spoke,
        reserve_id: position.reserve_id,
    })
}
