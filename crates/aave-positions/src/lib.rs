//! Aave v4 positions: what a wallet holds, and what it is worth.
//!
//! Today this is the arithmetic alone — the folds and the read stores follow in
//! Phase 2. See `docs/rust-migration.md`.
//!
//! **Every arithmetic operation in this crate is checked.** The workspace lint
//! block deliberately leaves `arithmetic_side_effects` off, because a loop
//! counter is not where silent wrapping costs anything; here it is the whole
//! point, so the lint lives beside the code it governs. What it buys is that a
//! product which leaves `uint256` becomes a [`valuation::Error`] rather than a
//! number: every quantity these formulas touch is a `uint256` on chain, so a
//! value outside that range is not a state the protocol can be in, and the
//! type says so rather than a comment.
//! **The surface is what a caller outside the crate uses, and no more.** The
//! contracts' math libraries are a private module, and `pub(super)` inside it:
//! `valuation` is the only consumer they have or should get, and the private
//! module already blocks a sibling either way — so the modifier is what keeps
//! that true if the module's own visibility ever widens. A caller wanting
//! `ceil(a * b / RAY)` wants [`valuation::PositionShares::value_at`] instead.
//! [`valuation::to_value`] is `pub` because pricing is a separate call the API
//! makes; `value_at` and the three types are `pub` only until Phase 2's store
//! is their in-crate consumer — narrowing them today makes the whole module
//! dead code, since nothing but the tests calls it yet.
#![deny(clippy::arithmetic_side_effects)]

pub mod valuation;
