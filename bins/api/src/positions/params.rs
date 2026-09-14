//! What a caller may ask for, and what happens when they ask for something else.
//!
//! **Every fault at once, not the first**, which is what `env` already does for
//! configuration here. The service this replaces reports one — its path and
//! query run through separate pipes — so the behaviour is kept and the wording
//! is not.
//!
//! **Unrecognised query parameters are refused.** `?limt=200` is a 400, because
//! the cost of being lenient is a caller who believes a parameter took effect.
//!
//! The bounds are constants: the Main Spoke lists fourteen reserves, so paging
//! is a contract formality and a knob nobody turns only ever goes wrong.

use std::collections::HashMap;

use alloy_primitives::Address;

use crate::errors::{self, BoxedAppError};

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

/// 2026-03-23T14:45:59Z — the Main Spoke's first log, and the earliest instant
/// any position can be valued at.
const GENESIS: u64 = 1_774_277_159;

/// 2100-01-01: a **units check rather than a policy** on how far ahead a caller
/// may value. `as_of` in milliseconds is past the floor above, so unbounded it
/// would extrapolate the index tens of thousands of years and answer a page of
/// enormous numbers with nothing to say they are wrong.
const MAX_AS_OF: u64 = 4_102_444_800;

/// The four this endpoint answers to. Anything else is a refusal rather than a
/// value nobody reads.
const KNOWN: [&str; 4] = ["spoke", "limit", "cursor", "as_of"];

/// One request, parsed.
pub(crate) struct Listing {
    pub(crate) chain_id: u32,
    pub(crate) user: Address,
    /// `None` lists every Spoke this wallet has touched.
    pub(crate) spoke: Option<Address>,
    pub(crate) limit: u32,
    /// Still opaque here. Verifying it needs the scope, which needs the three
    /// fields above, so the codec is handed it once they exist.
    pub(crate) cursor: Option<String>,
    /// Unix seconds to read and value the page at. `None` is now.
    pub(crate) as_of: Option<u64>,
}

impl Listing {
    /// # Errors
    ///
    /// A 400 naming every parameter that was wrong.
    pub(crate) fn parse(chain_id: &str, user: &str, query: &str) -> Result<Self, BoxedAppError> {
        let mut reading = Reading::new(query);

        let listing = Self {
            chain_id: reading.positive("chain_id", chain_id),
            user: reading.address("user", user),
            spoke: reading.optional_address("spoke"),
            limit: reading.bounded("limit", DEFAULT_LIMIT, 1, MAX_LIMIT),
            cursor: reading.cursor(),
            as_of: reading.optional_bounded("as_of", GENESIS, MAX_AS_OF),
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
    /// Reads the query string, refusing a key this endpoint does not answer to
    /// and a key given twice — `?limit=1&limit=2` is a caller who does not know
    /// what they asked for, and either resolution is a guess.
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
    fn finish(self) -> Result<(), BoxedAppError> {
        if self.faults.is_empty() {
            Ok(())
        } else {
            // One line rather than the `\n` its predecessor emits: this goes
            // into a JSON string, where a newline is an escape a terminal shows
            // literally.
            Err(errors::bad_request(self.faults.join("; ")))
        }
    }

    fn positive(&mut self, name: &str, value: &str) -> u32 {
        match value.parse::<u32>() {
            Ok(parsed) if parsed > 0 => parsed,
            _ => {
                self.reject(format!("{name} must be a positive integer, got {value:?}"));
                0
            }
        }
    }

    /// The value that is discarded on a fault is [`Address::ZERO`], which never
    /// reaches a query: [`Reading::finish`] refuses before the listing is used.
    fn address(&mut self, name: &str, value: &str) -> Address {
        value.parse().unwrap_or_else(|_| {
            self.reject(format!(
                "{name} must be a 20-byte hex address, got {value:?}"
            ));
            Address::ZERO
        })
    }

    /// Checksummed or lower-case, either matches: `Address` is the check and is
    /// case-insensitive, and the adapter lower-cases into the query anyway.
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

    /// Empty is refused rather than read as absent: a caller sending `?cursor=`
    /// meant to send one, and serving the first page instead would answer a
    /// question they did not ask.
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
    use crate::test_support::answered;

    /// Checksummed, as every block explorer hands them back.
    const ALICE: &str = "0x82D16fF1C724ab72F218A3f7f6DD3E5385ee87E8";
    const SPOKE: &str = "0x94e7A5dCbE816e498b89aB752661904E2F56c485";

    fn parse(query: &str) -> Result<Listing, BoxedAppError> {
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
        // The Spoke is genuinely optional: a caller asking what a wallet holds
        // should not have to know which Spokes exist.
        let listing = parse("").expect("a bare listing");

        assert_eq!(listing.chain_id, 1);
        assert_eq!(listing.limit, DEFAULT_LIMIT);
        assert_eq!(listing.spoke, None);
        assert_eq!(listing.cursor, None);
        assert_eq!(listing.as_of, None);
    }

    #[test]
    fn reads_a_checksummed_address_off_a_block_explorer() {
        // Both halves, because the type is what makes the case go away: the
        // fold stores one spelling and `Address` holds the twenty bytes, so the
        // lower-casing its predecessor did at the edge has nothing left to do.
        let listing = parse(&format!("spoke={SPOKE}")).expect("checksummed is an address");

        assert_eq!(
            Some(listing.user),
            ALICE.to_lowercase().parse::<Address>().ok()
        );
        assert_eq!(listing.spoke, SPOKE.to_lowercase().parse::<Address>().ok());
    }

    #[test]
    fn refuses_a_chain_id_that_is_not_one() {
        // Coercing failing open is the danger: `Number('abc')` is NaN and
        // `Number('')` is 0, so without the bounds `/chains/abc/…` would be a
        // query for a chain that does not exist rather than an error.
        for chain_id in ["abc", "", "-1", "1.5", "0"] {
            assert!(
                refused(chain_id, ALICE, "").contains("chain_id must be a positive integer"),
                "{chain_id:?}"
            );
        }
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
        // so a spelling the anchored regex refused now resolves. Twenty bytes
        // are twenty bytes, so it is pinned rather than guarded against.
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
            Some(Some(1_785_000_000))
        );

        // The likeliest wrong value by far. Unbounded it would value every
        // position tens of thousands of years out — which is not an error, just
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
            Some(Some(GENESIS))
        );
        assert_eq!(
            parse("as_of=4102444800").ok().map(|l| l.as_of),
            Some(Some(MAX_AS_OF))
        );
    }

    #[test]
    fn refuses_a_parameter_it_does_not_recognise() {
        // `?limt=200` would otherwise strip silently and serve the default, so
        // the caller reads a page size they never asked for and believes they
        // set one.
        assert_eq!(
            refused("1", ALICE, "limt=200"),
            r#"unrecognized query parameter "limt""#
        );
    }

    #[test]
    fn refuses_the_spelling_its_predecessor_answered_to() {
        // `asOf` is the one parameter this port renames, so the URL that works
        // against the service beside it is a refusal here rather than an
        // instant silently ignored.
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
        // The behaviour kept from the service this replaces, which reports one:
        // its path and query run through separate pipes and the first to throw
        // wins. Four here, in the order the parameters are read.
        let message = refused("0", "0xnope", "limt=1&limit=999");

        assert_eq!(
            message,
            concat!(
                r#"unrecognized query parameter "limt"; "#,
                r#"chain_id must be a positive integer, got "0"; "#,
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
