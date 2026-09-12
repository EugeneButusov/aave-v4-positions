/// What a token calls itself, as the read path needs it.
///
/// **A null label is an answer; a missing row is not.** `symbol` and `name` are
/// OPTIONAL in EIP-20, so a token that has neither is conformant, and the row
/// existing is what records that the question was put. Both states are kept
/// apart by presence in the map rather than by these fields — see
/// [`TokenMetadataStore::labels`](crate::TokenMetadataStore::labels) — which is
/// what stops a mute token being re-read on every sweep forever.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenLabel {
    pub symbol: Option<String>,
    pub name: Option<String>,
}
