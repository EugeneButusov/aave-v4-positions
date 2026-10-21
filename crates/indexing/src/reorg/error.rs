//! What the detector and its window refuse with.

/// One enum for both ports, because they are never held apart: a detector owns
/// a window, and every call into either lands in the same place — the loop,
/// which retries all of it.
///
/// [`Chain`](Self::Chain) is the detector's alone and [`Malformed`](Self::Malformed)
/// the window adapter's; a reader of either signature can see which from the
/// variant.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// **Opaque rather than this adapter's driver error**, unlike
    /// [`cursor::Error`](crate::Error). That port has one implementation and
    /// this one has two that must both be able to refuse: the detector's
    /// contract says a failed `append` surfaces rather than reporting a block it
    /// does not hold, and only a store that refuses can show it does.
    #[error("the header window could not be read or written")]
    Failed {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// A column held something its type says it cannot.
    ///
    /// Unreachable from a table the migrations built: `hash` and `parent_hash`
    /// each carry a `CHECK (… ~ '^0x[0-9a-f]{64}$')`. It refuses rather than
    /// substitutes, because a window read positionally would place a fork from
    /// a header it had to invent.
    #[error("{column} is not a {expected}: {value}")]
    Malformed {
        column: &'static str,
        expected: &'static str,
        value: String,
    },

    /// Walking a fork and vetting a resume point both read headers.
    #[error("the chain could not be read while placing a fork")]
    Chain(#[from] crate::chain::Error),
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
