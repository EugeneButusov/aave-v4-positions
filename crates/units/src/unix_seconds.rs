use std::fmt;

/// An instant, in whole seconds since the Unix epoch.
///
/// A tag, not a bound: what counts as a sensible instant is each caller's
/// policy. What it buys is that a block height cannot be assigned to one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnixSeconds(u64);

impl UnixSeconds {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// For arithmetic and `const`s; a boundary wants [`u64::from`].
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for UnixSeconds {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<UnixSeconds> for u64 {
    fn from(instant: UnixSeconds) -> Self {
        instant.0
    }
}

impl fmt::Display for UnixSeconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

#[cfg(test)]
mod tests {
    use super::UnixSeconds;

    #[test]
    fn orders_by_the_instant_it_carries() {
        let earlier = UnixSeconds::new(1_774_277_159);
        let later = UnixSeconds::new(1_785_000_000);

        assert!(earlier < later);
        assert_eq!(earlier.max(later), later);
    }

    #[test]
    fn survives_the_round_trip_a_boundary_makes() {
        assert_eq!(u64::from(UnixSeconds::from(1_785_000_000)), 1_785_000_000);
    }

    #[test]
    fn renders_as_the_number_a_query_string_carries() {
        assert_eq!(UnixSeconds::new(1_785_000_000).to_string(), "1785000000");
    }
}
