//! What the detector and its window refuse with.

/// Errors from the detector and its owned header window.
///
/// Postgres errors convert explicitly because read and write failures differ.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the header window read failed")]
    WindowReadFailed(#[source] postgres::Error),

    #[error("the header window could not be written")]
    WindowWriteFailed(#[source] postgres::Error),

    #[error("the chain could not be read while placing a fork")]
    ChainReadFailed(#[from] crate::chain::Error),

    /// Invalid stored data cannot safely participate in an ancestor walk.
    #[error("{column} is not a {expected}: {value}")]
    Malformed {
        column: &'static str,
        expected: &'static str,
        value: String,
    },

    #[cfg(test)]
    #[error("the reorg window refused to retain a processed header")]
    RetentionRefused,

    #[cfg(test)]
    #[error("the reorg window refused to apply a rewind")]
    RewindRefused,
}
