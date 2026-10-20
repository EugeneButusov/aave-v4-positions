//! One wallet's positions, on one chain.
//!
//! `GET /{prefix}/v1/chains/{chain_id}/users/{user}/positions`, ordered by Spoke
//! then reserve, paged by keyset. Positions from different Spokes may be listed
//! together but never summed: each Spoke is an isolated margin account (§12.3),
//! so a total across two hides an imminent liquidation behind unrelated
//! collateral.

mod cursor;
mod list;
mod params;
mod views;

pub(crate) use cursor::{MIN_SECRET_BYTES, Signer};
pub(crate) use list::routes;
