//! What a cursor read or write refuses with.

/// A cursor that cannot be read, or an advance that cannot be recorded.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// **Opaque rather than this adapter's driver error**, for the reason
    /// [`reorg::Error`](crate::ReorgError) gives: the write port has a second
    /// implementation that must be able to refuse, because "a rejected cursor
    /// save re-dispatches the same range" is a contract clause and only a store
    /// that rejects can show it holds.
    ///
    /// Not `ReadFailed`, as its two sibling crates spell it: this is the one of
    /// the three whose port also writes.
    #[error("the cursor could not be read or written")]
    Failed {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A column held something its type says it cannot.
    ///
    /// Unreachable from a table the migrations built: `last_hash` carries a
    /// `CHECK (last_hash ~ '^0x[0-9a-f]{64}$')`.
    #[error("{column} is not a {expected}: {value}")]
    Malformed {
        column: &'static str,
        expected: &'static str,
        value: String,
    },
}

/// By hand rather than `#[from]`, so `?` still works over a boxed source.
impl From<postgres::Error> for Error {
    fn from(source: postgres::Error) -> Self {
        Self::Failed {
            source: Box::new(source),
        }
    }
}

impl Error {
    /// What an implementation that is not a database refuses with.
    #[cfg(test)]
    pub(crate) fn unavailable(what: &'static str) -> Self {
        Self::Failed {
            source: what.into(),
        }
    }
}
