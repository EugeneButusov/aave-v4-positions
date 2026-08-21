//! The ClickHouse implementation of [`PositionStore`](super::PositionStore).
//!
//! Everything storage-shaped is in here: `sql` is the statement, `row` is what
//! the server sends and what it means, `position_store` runs one and returns
//! the other, and `fixtures` is the migrated database its specs read from.
//!
//! The split above the port would be worth nothing if the specs stayed there:
//! every one of them drives a real server, so they live with the adapter they
//! exercise rather than beside the types they happen to assert on.

#[cfg(test)]
mod fixtures;
mod position_store;
mod row;
mod sql;

pub use position_store::ClickHousePositionStore;
