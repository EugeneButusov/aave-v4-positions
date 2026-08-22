//! The ClickHouse implementation of [`PositionStore`](super::PositionStore).
//!
//! Everything storage-shaped is in here: `sql` is the statement, `row` is what
//! the server sends and what it means, `position_store` runs one and returns
//! the other, and `harness` stands up the migrated database its specs read
//! from.
//!
//! The specs split the same way. What holds for every implementation is the
//! port's, in `store::contract`; what is left here either drives a
//! real server through this adapter's `Fixture` or asserts on a string this
//! module built.

#[cfg(test)]
mod harness;
mod position_store;
mod row;
mod sql;

pub use position_store::ClickHousePositionStore;
