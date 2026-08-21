//! Aave v4 positions: what a wallet holds, and what it is worth.
//!
//! Two modules. [`valuation`] is the arithmetic, transcribed from the
//! contracts; [`store`] is the read side over the fold, and the only thing that
//! calls it. Nothing here writes — the fold is materialized views over the
//! event ledger, which `bins/indexer` fills.
//!
//! **Every arithmetic operation in this crate is checked.** The workspace lint
//! block deliberately leaves `arithmetic_side_effects` off, because a loop
//! counter is not where silent wrapping costs anything; here it is the whole
//! point, so the lint lives beside the code it governs. What it buys is that a
//! product which leaves `uint256` becomes a [`valuation::Error`] rather than a
//! number: every quantity these formulas touch is a `uint256` on chain, so a
//! value outside that range is not a state the protocol can be in, and the
//! type says so rather than a comment.
//!
//! **The surface is what a caller outside the crate uses, and no more.** The
//! contracts' math libraries are a private module and `pub(super)` inside it;
//! the asset state, a position's shares and the call that values them are
//! `pub(crate)`, because [`store`] is what builds and consumes all three. What
//! leaves the crate is a [`store::Position`], the [`valuation::Valuation`] on
//! it, and [`valuation::to_value`] — which stays public because pricing is a
//! separate call the API makes, on a number the store already returned.
#![deny(clippy::arithmetic_side_effects)]

pub mod store;
pub mod valuation;
