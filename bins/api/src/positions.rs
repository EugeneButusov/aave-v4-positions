//! One wallet's positions, on one chain.
//!
//! `GET /{prefix}/v1/chains/{chain_id}/users/{user}/positions`, and the whole of
//! what this service answers beyond its probes. Ordered by Spoke then reserve,
//! paged by keyset.
//!
//! [`list`] is the endpoint. [`params`] is what a caller may ask for, [`cursor`]
//! how a resume point is published and taken back, and [`scale`] the one
//! decision every number on the wire shares. A module declares its endpoints and
//! holds only what they share; the endpoint holds its own handler, its own types
//! and its own assembly.
//!
//! **Positions from different Spokes may be listed together but never summed.**
//! Each Spoke is an isolated margin account with its own collateral factors,
//! oracle and health factor (§12.3), so a total across two of them hides an
//! imminent liquidation behind unrelated collateral. Every row names the Spoke
//! it came from, and nothing here aggregates.

mod cursor;
mod list;
mod params;
mod scale;
mod wire;

pub(crate) use cursor::{MIN_SECRET_BYTES, Signer};
pub(crate) use list::routes;
