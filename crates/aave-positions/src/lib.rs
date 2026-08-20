//! Aave v4 positions: what a wallet holds, and what it is worth.
//!
//! Today this is the arithmetic alone — the folds and the read stores follow in
//! Phase 2. See `docs/rust-migration.md`.
//!
//! **Every arithmetic operation in this crate is checked.** The workspace lint
//! block deliberately leaves `arithmetic_side_effects` off, because a loop
//! counter is not where silent wrapping costs anything; here it is the whole
//! point, so the lint lives beside the code it governs. What it buys is that a
//! product which leaves `uint256` becomes an [`valuation::Error`] rather than a
//! number, and the widths the TypeScript never had to carry are enforced rather
//! than described.
#![deny(clippy::arithmetic_side_effects)]

pub mod valuation;
