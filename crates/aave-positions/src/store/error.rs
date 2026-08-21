//! What a read refuses with.

use crate::valuation;

/// A page that cannot be served.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The server refused the query, or the connection did.
    #[error("the position query failed")]
    Query(#[from] clickhouse_client::clickhouse::error::Error),

    /// A column held something its type says it cannot.
    ///
    /// Every wide integer arrives as a decimal string, because the alternative
    /// is a JSON number that has already lost its tail past 2^53 (§7.5) — so
    /// parsing is where a corrupt fold surfaces. Unreachable from a fold the
    /// migrations built: `toInt256` throws on the insert rather than storing
    /// something unparseable.
    #[error("{column} is not a {expected}: {value}")]
    Malformed {
        column: &'static str,
        expected: &'static str,
        value: String,
    },

    /// The arithmetic refused. See [`valuation::Error`].
    ///
    /// A page fails rather than reporting the position unvalued, because both
    /// reachable variants mean the inputs disagree with each other rather than
    /// that a number is missing: a checkpoint ahead of `as_of`, or a premium
    /// the fold has driven below zero.
    #[error("a position on this page cannot be valued")]
    Valuation(#[from] valuation::Error),
}
