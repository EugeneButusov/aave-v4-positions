//! What a chain read refuses with.

/// Two variants, split where a caller acts differently: a processor answers
/// [`RangeTooLarge`](Self::RangeTooLarge) by halving the range it asks for next
/// and retries anything else unchanged. Which one a provider's refusal is
/// counts as the adapter's judgement.
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
