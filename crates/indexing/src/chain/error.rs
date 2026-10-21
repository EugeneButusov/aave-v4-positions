//! What a chain read refuses with.

/// Two variants, and the split is the one a caller has to act on.
///
/// [`RangeTooLarge`](Self::RangeTooLarge) is named because a processor answers
/// it by halving the range it asks for next; everything else is undifferentiated
/// on purpose — the loop retries any of it, unbounded, and a taxonomy it never
/// reads would be a taxonomy nothing keeps honest. Which case a provider's
/// refusal falls into is the adapter's judgement, not this type's.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Both ends inclusive, as the filter gave them.
    #[error("the provider refused the range {from}..={to}: {detail}")]
    RangeTooLarge { from: u64, to: u64, detail: String },

    #[error("the chain could not be read")]
    Unavailable {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}
