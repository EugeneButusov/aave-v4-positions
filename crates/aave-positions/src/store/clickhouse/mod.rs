//! The ClickHouse implementation of [`PositionStore`](super::PositionStore).
//!
//! What holds for every implementation is the port's, in `store::conformance`;
//! what is left here drives a real server or asserts on a string built below.

#[cfg(test)]
mod harness;
mod position_store;
mod row;
mod sql;

pub use position_store::ClickHousePositionStore;
