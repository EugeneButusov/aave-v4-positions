use std::{error::Error, fmt, str::FromStr};

/// EIP-2294's ceiling: chain ids are capped at `MAX_SAFE_INTEGER` so that every
/// JSON-RPC implementation can carry one in a double.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Which chain a row, a cursor or a request belongs to.
///
/// Wider than it looks like it needs to be: deployed chain ids already exceed
/// `u32` — Palm is `11297108109` — so a narrower type refuses a valid chain as
/// if the caller had malformed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ChainId(u64);

impl ChainId {
    /// The lowest chain id there is. Zero is not one: EIP-155 gives it to
    /// unprotected transactions, and no network claims it.
    pub const MIN: Self = Self(1);

    /// EIP-2294's ceiling.
    pub const MAX: Self = Self(MAX_SAFE_INTEGER);

    /// # Errors
    ///
    /// Zero, or past [`ChainId::MAX`].
    pub const fn new(value: u64) -> Result<Self, NotAChainId> {
        if value == 0 || value > MAX_SAFE_INTEGER {
            return Err(NotAChainId);
        }

        Ok(Self(value))
    }

    /// A chain id from a literal, where [`ChainId::new`]'s `Result` cannot be
    /// unwrapped — a `const`, or a test fixture naming the chain it describes.
    ///
    /// In a `const` a bad literal fails to compile. Called at run time it
    /// panics instead, so it takes literals and not arguments.
    #[must_use]
    #[expect(
        clippy::panic,
        reason = "a const literal outside the range is a compile error, which is the point"
    )]
    pub const fn known(value: u64) -> Self {
        match Self::new(value) {
            Ok(id) => id,
            Err(_) => panic!("not a chain id"),
        }
    }

    /// The number itself, for arithmetic and for a `const`. A conversion at a
    /// boundary wants [`u64::from`] instead.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<ChainId> for u64 {
    fn from(id: ChainId) -> Self {
        id.0
    }
}

impl TryFrom<u64> for ChainId {
    type Error = NotAChainId;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl FromStr for ChainId {
    type Err = NotAChainId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse::<u64>().map_err(|_| NotAChainId)?.try_into()
    }
}

impl fmt::Display for ChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A number that is not a chain id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotAChainId;

impl fmt::Display for NotAChainId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "must be 1..={MAX_SAFE_INTEGER}")
    }
}

impl Error for NotAChainId {}

#[cfg(test)]
mod tests {
    use super::{ChainId, NotAChainId};

    #[test]
    fn carries_a_chain_id_too_wide_for_the_type_this_replaces() {
        let palm = ChainId::new(11_297_108_109).expect("a deployed chain id");

        assert_eq!(u64::from(palm), 11_297_108_109);
    }

    #[test]
    fn refuses_a_chain_id_past_the_protocol_bound() {
        assert_eq!(ChainId::new(ChainId::MAX.get() + 1), Err(NotAChainId));
    }

    #[test]
    fn refuses_zero_because_no_network_claims_it() {
        assert_eq!(ChainId::new(0), Err(NotAChainId));
    }

    #[test]
    fn takes_both_ends_of_the_range() {
        assert!(ChainId::new(ChainId::MIN.get()).is_ok());
        assert!(ChainId::new(ChainId::MAX.get()).is_ok());
    }

    /// Pins the footgun its doc names: outside a `const` there is no compiler
    /// to catch the literal, so it is a panic rather than a silent chain 1.
    #[test]
    #[should_panic(expected = "not a chain id")]
    fn panics_on_a_literal_it_cannot_refuse_at_compile_time() {
        let _ = ChainId::known(0);
    }

    /// The ceiling is EIP-2294's and not a width borrowed from whatever
    /// happens to store one, so it is asserted rather than left to the doc
    /// that names it.
    #[test]
    fn stops_where_the_protocol_does() {
        assert_eq!(ChainId::MAX.get(), (1 << 53) - 1);
        assert_eq!(ChainId::MAX.get(), 9_007_199_254_740_991);
    }

    #[test]
    fn reads_a_path_segment() {
        assert_eq!("1".parse::<ChainId>(), ChainId::new(1));
        assert_eq!(
            "11297108109".parse::<ChainId>(),
            ChainId::new(11_297_108_109)
        );
    }

    #[test]
    fn refuses_a_segment_that_is_not_a_number() {
        for segment in ["", "0x1", "-1", "1.0", " 1", "9007199254740992"] {
            assert_eq!(segment.parse::<ChainId>(), Err(NotAChainId), "{segment:?}");
        }
    }

    #[test]
    fn takes_a_literal_in_a_const() {
        const MAINNET: ChainId = ChainId::known(1);

        assert_eq!(MAINNET, ChainId::MIN);
    }

    #[test]
    fn says_the_range_it_wanted() {
        assert_eq!(NotAChainId.to_string(), "must be 1..=9007199254740991");
    }
}
