//! The page itself: what wraps the positions, and the two clocks beside them.

use aave_positions::store::{Position, PositionPage};
use indexing::SyncStatus;
use serde::Serialize;
use time::OffsetDateTime;
use utoipa::ToSchema;

use super::{Error, Item, Labels, Prices, price_for};
use crate::config::Staleness;

/// One wallet's positions, valued at one instant.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Page {
    /// How current this payload is. Present on every page: amounts are
    /// per-block quantities, so a sync that differs from the previous page is
    /// the signal that the indexer advanced mid-walk.
    sync: Progress,

    /// When every amount on this page was computed — one instant for the whole
    /// page, defaulting to now. Distinct from `sync.last_block`: the shares are
    /// as far as the indexer has folded, the amounts are those shares valued
    /// here. The `as_of` query parameter that sets it is Unix seconds.
    #[serde(with = "time::serde::rfc3339")]
    valued_at: OffsetDateTime,

    /// How current the prices behind this page are — a third clock beside
    /// `sync` and `valued_at`, because prices have their own cadence. Null when
    /// nothing here is priced, and whenever `as_of` is set: amounts are
    /// extrapolated to that instant and prices are not.
    #[schema(required = true)]
    pricing: Option<Pricing>,

    items: Vec<Item>,

    /// Hand back verbatim as `cursor` to fetch the next page; `null` is the
    /// last page. Opaque and signed, and only valid for the listing that issued
    /// it — changing the wallet, chain or Spoke filter is refused.
    #[schema(required = true)]
    next_cursor: Option<String>,
}

impl Page {
    /// # Errors
    ///
    /// [`Error`], when a position will not render or the store valued the page
    /// at a number no date can hold.
    pub(crate) fn new(
        sync: &SyncStatus,
        page: &PositionPage,
        labels: &Labels,
        prices: &Prices,
        staleness: Staleness,
        next_cursor: Option<String>,
    ) -> Result<Self, Error> {
        let valued_at = i64::try_from(page.valued_at)
            .ok()
            .and_then(|seconds| OffsetDateTime::from_unix_timestamp(seconds).ok())
            .ok_or(Error::ValuedAt(page.valued_at))?;

        Ok(Self {
            sync: Progress::new(sync, staleness.sync),
            valued_at,
            pricing: Pricing::new(&page.items, prices, staleness.price),
            items: page
                .items
                .iter()
                .map(|position| Item::new(position, labels, prices))
                .collect::<Result<_, _>>()?,
            next_cursor,
        })
    }
}

/// How far the indexer has got on this chain.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct Progress {
    /// The last block the indexer processed on this chain.
    last_block: u64,

    /// The hash the indexer saw at that height.
    last_block_hash: String,

    /// When the indexer last advanced, by the database clock that recorded it.
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,

    /// Seconds since the indexer last advanced. Measured on the server that
    /// wrote the timestamp, so it is not affected by clock skew between hosts.
    age_seconds: u64,

    /// Whether `age_seconds` has passed this deployment's threshold. A stale
    /// response is still served: the shares were folded as of `last_block`, and
    /// `valued_at` says which instant they were valued at.
    stale: bool,
}

impl Progress {
    fn new(sync: &SyncStatus, stale_after: u64) -> Self {
        Self {
            last_block: sync.last_block,
            last_block_hash: sync.last_hash.to_string(),
            updated_at: sync.updated_at,
            age_seconds: sync.age_seconds,
            stale: sync.age_seconds > stale_after,
        }
    }
}

/// How current the prices behind this page are.
#[derive(Debug, Serialize, ToSchema)]
struct Pricing {
    /// When the oldest price behind any number on this page was read.
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,

    /// Seconds since then, measured by the database clock rather than this
    /// process's, so clock skew between the two cannot be reported as staleness.
    age_seconds: u64,

    /// Whether that exceeds this deployment's threshold. It measures how long
    /// since we last read the oracle, never since a feed last moved — an hour
    /// without a Chainlink update is normal.
    stale: bool,
}

impl Pricing {
    /// The page's price clock, from the oldest price behind it — what a caller
    /// needs is how far to trust the worst number in front of them. It diverges
    /// only when the oracle refused a reserve and its last price was left to
    /// age. Ties keep the earlier position, which the page's order makes
    /// deterministic.
    fn new(positions: &[Position], prices: &Prices, stale_after: u64) -> Option<Self> {
        positions
            .iter()
            .filter_map(|position| price_for(position, prices))
            .reduce(|worst, price| {
                if price.age_seconds > worst.age_seconds {
                    price
                } else {
                    worst
                }
            })
            .map(|price| Self {
                updated_at: price.priced_at,
                age_seconds: price.age_seconds,
                stale: price.age_seconds > stale_after,
            })
    }
}
