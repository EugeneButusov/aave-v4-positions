//! What a caller may ask for, and what happens when they ask for something else.
//!
//! Every fault at once, not the first. The service this replaces reports one,
//! so the behaviour is kept and the wording is not.
//!
//! Unrecognised query parameters are refused: `?limt=200` is a 400, because the
//! cost of being lenient is a caller who believes it took effect.
//!
//! The bounds are constants — the Main Spoke lists fourteen reserves.

use std::collections::HashMap;

use alloy_primitives::Address;
use units::{ChainId, UnixSeconds};

use crate::errors::ApiError;

pub(crate) const DEFAULT_LIMIT: u32 = 50;
pub(crate) const MAX_LIMIT: u32 = 200;

/// 2026-03-23T14:45:59Z — the Main Spoke's first log, and the earliest instant
/// any position can be valued at.
pub(crate) const GENESIS: u64 = 1_774_277_159;

/// 2100-01-01: a units check rather than a policy. `as_of` in milliseconds is
/// past the floor above, so unbounded it would extrapolate the index tens of
/// thousands of years and answer numbers with nothing to say they are wrong.
///
/// **It is not the real ceiling.** That is the block this deployment has folded
/// to, which only the handler knows — this runs before any read and the
/// published `maximum` keyword has to be a constant.
pub(crate) const MAX_AS_OF: u64 = 4_102_444_800;

/// The four this endpoint answers to; anything else is a refusal.
const KNOWN: [&str; 4] = ["spoke", "limit", "cursor", "as_of"];

/// `default` takes a function to call, not a constant to read, so this is what
/// keeps the published default and the parsed one the same number.
fn default_limit() -> u32 {
    DEFAULT_LIMIT
}

/// The four query parameters, as the document describes them.
///
/// Never constructed: [`Listing::parse`] reads the query string itself, because
/// it reports every fault at once and has to see the keys this endpoint does
/// not answer to, neither of which survives a deserialize into a struct. This
/// type exists because `IntoParams` is the only form that publishes a
/// `default`, which the inline `params(...)` attribute cannot express — so a
/// caller reading the document learns the page size it gets by saying nothing.
///
/// It lives beside the parser rather than beside the route for the obvious
/// reason: the two have to agree, and
/// `documents_the_bounds_the_parser_enforces` is what says they do.
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
#[expect(dead_code, reason = "the derive reads the field names, not the values")]
pub(crate) struct QueryParameters {
    /// Narrow to one Spoke. Omit to list every Spoke this wallet has touched.
    #[param(example = "0x94e7a5dcbe816e498b89ab752661904e2f56c485")]
    spoke: Option<String>,

    /// How many positions to return.
    #[param(minimum = 1, maximum = 200, default = default_limit)]
    limit: Option<u32>,

    /// A `next_cursor` from a previous page, handed back verbatim. Only valid
    /// for the listing that issued it.
    cursor: Option<String>,

    /// Unix seconds to value the page at. Defaults to the block timestamp of
    /// the block this chain is indexed to — amounts accrue every second and the
    /// chain emits nothing while they do, so naming the instant is what makes a
    /// response reproducible. Echoed back as `valued_at`. The bounds published
    /// here are a units check; an instant later than the indexed block is
    /// refused as well.
    #[param(minimum = 1_774_277_159_u64, maximum = 4_102_444_800_u64)]
    as_of: Option<u64>,
}

pub(crate) struct Listing {
    pub(crate) chain_id: ChainId,
    pub(crate) user: Address,
    /// `None` lists every Spoke this wallet has touched.
    pub(crate) spoke: Option<Address>,
    pub(crate) limit: u32,
    /// Still opaque: verifying it needs the scope, which needs the three
    /// fields above.
    pub(crate) cursor: Option<String>,
    /// The instant to read and value the page at. `None` leaves it to the
    /// handler, which has the indexed block this defaults to.
    pub(crate) as_of: Option<UnixSeconds>,
}

impl Listing {
    /// # Errors
    ///
    /// A 400 naming every parameter that was wrong.
    pub(crate) fn parse(chain_id: &str, user: &str, query: &str) -> Result<Self, ApiError> {
        let mut reading = Reading::new(query);

        let listing = Self {
            chain_id: reading.chain_id("chain_id", chain_id),
            user: reading.address("user", user),
            spoke: reading.optional_address("spoke"),
            limit: reading.bounded("limit", DEFAULT_LIMIT, 1, MAX_LIMIT),
            cursor: reading.cursor(),
            as_of: reading
                .optional_bounded("as_of", GENESIS, MAX_AS_OF)
                .map(UnixSeconds::new),
        };

        reading.finish()?;
        Ok(listing)
    }
}

/// The query as a map, and what has been wrong with the request so far.
struct Reading {
    query: HashMap<String, String>,
    faults: Vec<String>,
}

impl Reading {
    /// Refuses a key this endpoint does not answer to, and a key given twice:
    /// either resolution of `?limit=1&limit=2` is a guess.
    fn new(query: &str) -> Self {
        let mut reading = Self {
            query: HashMap::new(),
            faults: Vec::new(),
        };

        for (key, value) in form_urlencoded::parse(query.as_bytes()) {
            if !KNOWN.contains(&key.as_ref()) {
                reading.reject(format!("unrecognized query parameter {key:?}"));
            } else if reading
                .query
                .insert(key.clone().into_owned(), value.into_owned())
                .is_some()
            {
                reading.reject(format!("query parameter {key:?} was given more than once"));
            }
        }

        reading
    }

    /// # Errors
    ///
    /// A 400 listing every fault, in the order the parameters are read.
    fn finish(self) -> Result<(), ApiError> {
        if self.faults.is_empty() {
            Ok(())
        } else {
            // One line rather than the `\n` its predecessor emits, which in a
            // JSON string is an escape a terminal shows literally.
            Err(ApiError::bad_request(self.faults.join("; ")))
        }
    }

    /// [`ChainId::MIN`] on a fault, which never reaches a query because
    /// [`Reading::finish`] refuses first.
    fn chain_id(&mut self, name: &str, value: &str) -> ChainId {
        value.parse().unwrap_or_else(|fault| {
            self.reject(format!("{name} {fault}, got {value:?}"));
            ChainId::MIN
        })
    }

    /// [`Address::ZERO`] on a fault, which never reaches a query because
    /// [`Reading::finish`] refuses first.
    fn address(&mut self, name: &str, value: &str) -> Address {
        value.parse().unwrap_or_else(|_| {
            self.reject(format!(
                "{name} must be a 20-byte hex address, got {value:?}"
            ));
            Address::ZERO
        })
    }

    /// Checksummed or lower-case: `Address` is the check and is
    /// case-insensitive.
    fn optional_address(&mut self, name: &str) -> Option<Address> {
        let value = self.query.get(name)?.clone();

        Some(self.address(name, &value))
    }

    fn bounded(&mut self, name: &str, default: u32, min: u32, max: u32) -> u32 {
        let Some(value) = self.query.get(name).cloned() else {
            return default;
        };

        match value.parse::<u32>() {
            Ok(parsed) if (min..=max).contains(&parsed) => parsed,
            _ => {
                self.reject(format!("{name} must be {min}..={max}, got {value:?}"));
                default
            }
        }
    }

    fn optional_bounded(&mut self, name: &str, min: u64, max: u64) -> Option<u64> {
        let value = self.query.get(name)?.clone();

        match value.parse::<u64>() {
            Ok(parsed) if (min..=max).contains(&parsed) => Some(parsed),
            _ => {
                self.reject(format!("{name} must be {min}..={max}, got {value:?}"));
                None
            }
        }
    }

    /// Empty is refused rather than read as absent: a caller sending
    /// `?cursor=` meant to send one.
    fn cursor(&mut self) -> Option<String> {
        let value = self.query.get("cursor")?.clone();

        if value.is_empty() {
            self.reject("cursor must not be empty".to_owned());
            return None;
        }

        Some(value)
    }

    fn reject(&mut self, fault: String) {
        self.faults.push(fault);
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;
    use crate::test_support::app::answered;

    /// Checksummed, as every block explorer hands them back.
    const ALICE: &str = "0x82D16fF1C724ab72F218A3f7f6DD3E5385ee87E8";
    const SPOKE: &str = "0x94e7A5dCbE816e498b89aB752661904E2F56c485";

    fn parse(query: &str) -> Result<Listing, ApiError> {
        Listing::parse("1", ALICE, query)
    }

    fn refused(chain_id: &str, user: &str, query: &str) -> String {
        let error = Listing::parse(chain_id, user, query)
            .err()
            .expect("expected a refusal");

        error.to_string()
    }

    #[test]
    fn needs_nothing_but_the_path() {
        let listing = parse("").expect("a bare listing");

        assert_eq!(listing.chain_id, ChainId::known(1));
        assert_eq!(listing.limit, DEFAULT_LIMIT);
        assert_eq!(listing.spoke, None);
        assert_eq!(listing.cursor, None);
        assert_eq!(listing.as_of, None);
    }

    #[test]
    fn reads_a_checksummed_address_off_a_block_explorer() {
        // Both halves: `Address` holds twenty bytes, so the lower-casing its
        // predecessor did at the edge has nothing left to do.
        let listing = parse(&format!("spoke={SPOKE}")).expect("checksummed is an address");

        assert_eq!(
            Some(listing.user),
            ALICE.to_lowercase().parse::<Address>().ok()
        );
        assert_eq!(listing.spoke, SPOKE.to_lowercase().parse::<Address>().ok());
    }

    #[test]
    fn refuses_a_chain_id_that_is_not_one() {
        // `Number('abc')` is NaN and `Number('')` is 0, so without the bounds
        // `/chains/abc/…` would query a chain that does not exist.
        for chain_id in ["abc", "", "-1", "1.5", "0"] {
            assert!(
                refused(chain_id, ALICE, "").contains("chain_id must be 1..="),
                "{chain_id:?}"
            );
        }
    }

    /// The width is the point: this one does not fit the type the narrower
    /// reading used, and it is a chain somebody runs.
    #[test]
    fn accepts_palms_chain_id() {
        let listing = Listing::parse("11297108109", ALICE, "").expect("a deployed chain id");

        assert_eq!(listing.chain_id, ChainId::known(11_297_108_109));
    }

    #[test]
    fn refuses_a_chain_id_past_the_protocol_bound() {
        let past = (ChainId::MAX.get() + 1).to_string();

        assert!(
            refused(&past, ALICE, "").contains("chain_id must be 1..="),
            "{past}"
        );
    }

    #[test]
    fn refuses_a_wallet_that_is_not_an_address() {
        let long = format!("{ALICE}00");

        for user in ["0x82d16ff1", &long, &format!("0x{}", "z".repeat(40)), ""] {
            assert!(
                refused("1", user, "").contains("user must be a 20-byte hex address"),
                "{user:?}"
            );
        }
    }

    #[test]
    fn takes_twenty_bytes_of_hex_without_the_prefix() {
        // Measured, not intended: `Address::from_str` strips an optional `0x`,
        // so a spelling the anchored regex refused now resolves.
        let bare = ALICE.trim_start_matches("0x");
        let listing = Listing::parse("1", bare, "").expect("twenty bytes of hex");

        assert_eq!(
            Some(listing.user),
            ALICE.to_lowercase().parse::<Address>().ok()
        );
    }

    #[test]
    fn bounds_the_page_size() {
        assert_eq!(parse("limit=10").ok().map(|l| l.limit), Some(10));
        assert_eq!(parse("limit=200").ok().map(|l| l.limit), Some(MAX_LIMIT));

        for limit in ["0", "201", "1.5", "-1", ""] {
            assert!(
                refused("1", ALICE, &format!("limit={limit}")).contains("limit must be 1..=200"),
                "{limit:?}"
            );
        }
    }

    #[test]
    fn takes_an_as_of_and_refuses_one_that_is_obviously_milliseconds() {
        assert_eq!(
            parse("as_of=1785000000").ok().map(|l| l.as_of),
            Some(Some(UnixSeconds::new(1_785_000_000)))
        );

        // The likeliest wrong value, and unbounded it is not an error — just
        // a page of numbers nobody can tell is wrong.
        for as_of in ["1785000000000", "1", "0"] {
            assert!(
                refused("1", ALICE, &format!("as_of={as_of}"))
                    .contains("as_of must be 1774277159..=4102444800"),
                "{as_of:?}"
            );
        }
    }

    #[test]
    fn takes_the_instant_at_either_end_of_its_range() {
        assert_eq!(
            parse("as_of=1774277159").ok().map(|l| l.as_of),
            Some(Some(UnixSeconds::new(GENESIS)))
        );
        assert_eq!(
            parse("as_of=4102444800").ok().map(|l| l.as_of),
            Some(Some(UnixSeconds::new(MAX_AS_OF)))
        );
    }

    #[test]
    fn refuses_a_parameter_it_does_not_recognise() {
        // Otherwise `?limt=200` strips silently and serves the default.
        assert_eq!(
            refused("1", ALICE, "limt=200"),
            r#"unrecognized query parameter "limt""#
        );
    }

    #[test]
    fn refuses_the_spelling_its_predecessor_answered_to() {
        // The one parameter this port renames, so the URL that works against
        // the service beside it refuses rather than ignoring the instant.
        assert!(
            refused("1", ALICE, "asOf=1785000000")
                .contains(r#"unrecognized query parameter "asOf""#)
        );
    }

    #[test]
    fn refuses_a_parameter_given_twice_rather_than_picking_one() {
        assert_eq!(
            refused("1", ALICE, "limit=1&limit=2"),
            r#"query parameter "limit" was given more than once"#
        );
    }

    #[test]
    fn refuses_an_empty_cursor_rather_than_treating_it_as_absent() {
        // `?cursor=` is a caller who lost their cursor, not one starting over.
        assert_eq!(refused("1", ALICE, "cursor="), "cursor must not be empty");
    }

    #[test]
    fn names_every_fault_rather_than_the_first() {
        // The service this replaces reports one; four here, in reading order.
        let message = refused("0", "0xnope", "limt=1&limit=999");

        assert_eq!(
            message,
            concat!(
                r#"unrecognized query parameter "limt"; "#,
                r#"chain_id must be 1..=9007199254740991, got "0"; "#,
                r#"user must be a 20-byte hex address, got "0xnope"; "#,
                r#"limit must be 1..=200, got "999""#
            )
        );
    }

    #[tokio::test]
    async fn is_a_400_in_this_services_own_envelope() {
        let error = parse("limit=0").err().expect("expected a refusal");
        let (status, body) = answered(error).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            r#"{"message":"limit must be 1..=200, got \"0\"","error":"Bad Request","status_code":400}"#
        );
    }
}
