//! The cursor as this API publishes it.
//!
//! **Signing belongs to publishing, not to paging** — `aave-positions` owns
//! [`PositionKey`], and this is what makes one safe to hand to a stranger. Not
//! confidentiality: position data is public on chain. It keeps the encoding
//! opaque, and it stops a cursor being carried between listings, where a resume
//! point from one wallet is well-formed in another's and silently skips every
//! reserve below it. Hence the scope in the tag rather than in the cursor.
//!
//! HMAC rather than a JWT, which would add algorithm negotiation and `alg: none`
//! to protect two short strings.
//!
//! **The key must be identical across replicas**, or a cursor from one pod is
//! rejected by the next. The TypeScript service computes the same tag, so while
//! both run either accepts the other's.

use std::fmt;

use aave_positions::store::PositionKey;
use alloy_primitives::{Address, U256};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::digest::InvalidLength;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq as _;

type Keyed = Hmac<Sha256>;

/// The one separator. Every field it joins is an address, a decimal id or
/// [`ALL_SPOKES`], none of which can contain it — so the concatenation has one
/// reading, and no tag verifies with the boundary moved.
const SEP: char = '|';

/// Not a valid address, so it cannot collide with a Spoke genuinely filtered on.
const ALL_SPOKES: &str = "*";

/// 128 bits. Full SHA-256 would triple the cursor to no benefit.
const TAG_BYTES: usize = 16;

/// A key shorter than this is guessable. Enforced in `config`, where the
/// variable is read: a value is checked once, where it enters.
pub(crate) const MIN_SECRET_BYTES: usize = 32;

/// The listing a page belongs to.
///
/// **`spoke` is the filter that was applied, not the Spoke a row came from**,
/// and `None` spans every Spoke. An all-Spokes resume point is well-formed
/// inside a single-Spoke listing, so the two must not sign identically.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scope {
    pub(crate) chain_id: u32,
    pub(crate) user: Address,
    pub(crate) spoke: Option<Address>,
}

/// Signs and verifies page cursors for this deployment.
pub(crate) struct Signer {
    /// Keyed once at boot and cloned per use. `Hmac` is `Clone`, and the key
    /// schedule is the half worth not repeating.
    keyed: Keyed,
}

impl Signer {
    /// # Errors
    ///
    /// [`InvalidLength`], which HMAC never produces — RFC 2104 takes a key of
    /// any length. Propagated rather than unwrapped: an unconstructible arm is
    /// still not ours to panic on.
    pub(crate) fn new(secret: &str) -> Result<Self, InvalidLength> {
        Ok(Self {
            keyed: Keyed::new_from_slice(secret.as_bytes())?,
        })
    }

    pub(crate) fn encode(&self, scope: &Scope, key: &PositionKey) -> String {
        let payload = payload(key);
        let body = URL_SAFE_NO_PAD.encode(&payload);
        let tag = self.tag(scope, &payload);

        format!("{body}.{tag}")
    }

    /// Verifies before it parses.
    ///
    /// # Errors
    ///
    /// [`Invalid`], which says what was wrong with the cursor and nothing about
    /// what a caller should be told — that belongs to whoever is speaking HTTP.
    pub(crate) fn decode(&self, encoded: &str, scope: &Scope) -> Result<PositionKey, Invalid> {
        let (body, tag) = encoded.split_once('.').ok_or(Invalid::Shape)?;
        if tag.contains('.') {
            return Err(Invalid::Shape);
        }

        let payload = URL_SAFE_NO_PAD
            .decode(body)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or(Invalid::Payload)?;

        // Constant time, and `subtle` answers `false` for a length mismatch
        // rather than panicking. `==` returns on the first differing byte.
        if !bool::from(self.tag(scope, &payload).as_bytes().ct_eq(tag.as_bytes())) {
            return Err(Invalid::Signature);
        }

        let (spoke, reserve_id) = payload.split_once(SEP).ok_or(Invalid::Fields)?;

        Ok(PositionKey {
            spoke: spoke.parse().map_err(|_| Invalid::Spoke)?,
            reserve_id: decimal(reserve_id)?,
        })
    }

    /// Signs a payload this service would never build, through the real
    /// [`Signer::tag`], so a case can prove what a valid tag over one does.
    #[cfg(test)]
    fn sign(&self, scope: &Scope, payload: &str) -> String {
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(payload),
            self.tag(scope, payload)
        )
    }

    /// Every field, joined by the one separator none of them can contain.
    fn tag(&self, scope: &Scope, payload: &str) -> String {
        let spoke = scope
            .spoke
            .map_or_else(|| ALL_SPOKES.to_owned(), |spoke| format!("{:#x}", spoke));

        let mut keyed = self.keyed.clone();
        keyed.update(
            format!(
                "{}{SEP}{:#x}{SEP}{spoke}{SEP}{payload}",
                scope.chain_id, scope.user
            )
            .as_bytes(),
        );

        URL_SAFE_NO_PAD.encode(&keyed.finalize().into_bytes()[..TAG_BYTES])
    }
}

/// The resume point: what the sorting key leaves free once the scope pins
/// `(chain, user)`. The Spoke is signed even when the listing pinned it, which
/// is what stops the two listings sharing a cursor.
///
/// Lower-case hex, because `Address`'s own `Display` is EIP-55 checksummed and
/// would sign a different string for the same address.
fn payload(key: &PositionKey) -> String {
    format!("{:#x}{SEP}{}", key.spoke, key.reserve_id)
}

/// The digits of a reserve id, and only digits.
///
/// **`U256::from_str` is not this check**: measured, it answers `Ok(0)` for the
/// empty string and reads `0x` as hex — so a missing id would resume from the
/// start of the listing rather than refuse.
fn decimal(value: &str) -> Result<U256, Invalid> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Invalid::ReserveId);
    }

    U256::from_str_radix(value, 10).map_err(|_| Invalid::TooLarge)
}

/// What a cursor can be wrong about.
///
/// **Deliberately not a `std::error::Error`.** `errors`' blanket impl turns any
/// of those into a logged 500, and every variant here is the caller's input
/// being wrong — so a `?` that skipped the mapping would answer an operator page
/// over a query parameter. Without the impl it does not compile, and whoever
/// holds the HTTP vocabulary has to say what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Invalid {
    Shape,
    Payload,
    Signature,
    Fields,
    Spoke,
    ReserveId,
    TooLarge,
}

impl fmt::Display for Invalid {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str(match self {
            Self::Shape => "expected a payload and a tag",
            Self::Payload => "the payload is not base64url text",
            Self::Signature => "signature does not match this listing",
            Self::Fields => "expected a Spoke and a reserve id",
            Self::Spoke => "the Spoke is not an address",
            Self::ReserveId => "the reserve id is not a number",
            Self::TooLarge => "the reserve id does not fit uint256",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    const ALICE: &str = "0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8";
    const BOB: &str = "0xb8516f75dcf450b5b455b5114f5a92f6abd37dca";
    const SPOKE: &str = "0x94e7a5dcbe816e498b89ab752661904e2f56c485";
    const OTHER_SPOKE: &str = "0x973a023a77420ba610f06b3858ad991df6d85a08";

    fn address(hex: &str) -> Address {
        hex.parse().expect("a literal address")
    }

    fn signer(secret: &str) -> Signer {
        Signer::new(secret).expect("HMAC takes a key of any length")
    }

    fn scope() -> Scope {
        Scope {
            chain_id: 1,
            user: address(ALICE),
            spoke: Some(address(SPOKE)),
        }
    }

    /// The same wallet, listed across every Spoke rather than narrowed to one.
    fn all_spokes() -> Scope {
        Scope {
            spoke: None,
            ..scope()
        }
    }

    fn key() -> PositionKey {
        PositionKey {
            spoke: address(SPOKE),
            reserve_id: U256::from(13),
        }
    }

    /// Edits the payload and leaves the tag the caller was given.
    fn tamper(encoded: &str, payload: &str) -> String {
        let tag = encoded.split('.').nth(1).unwrap_or_default();

        format!("{}.{tag}", URL_SAFE_NO_PAD.encode(payload))
    }

    fn refusal(encoded: &str, scope: &Scope) -> Invalid {
        signer(SECRET)
            .decode(encoded, scope)
            .expect_err("expected a refusal")
    }

    #[test]
    fn round_trips_the_resume_point_it_was_built_from() {
        let signer = signer(SECRET);
        let encoded = signer.encode(&scope(), &key());

        assert_eq!(signer.decode(&encoded, &scope()).ok(), Some(key()));
    }

    #[test]
    fn carries_the_spoke_so_an_all_spokes_walk_knows_where_it_stopped() {
        // Without it, a resume point restarts at whichever Spoke sorts first.
        let signer = signer(SECRET);
        let key = PositionKey {
            spoke: address(OTHER_SPOKE),
            reserve_id: U256::from(3),
        };
        let encoded = signer.encode(&all_spokes(), &key);

        assert_eq!(signer.decode(&encoded, &all_spokes()).ok(), Some(key));
    }

    #[test]
    fn stays_url_safe_so_it_needs_no_escaping_in_a_query_string() {
        let encoded = signer(SECRET).encode(&scope(), &key());

        assert_eq!(encoded.matches('.').count(), 1, "{encoded}");
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')),
            "{encoded}"
        );
    }

    #[test]
    fn refuses_a_payload_edited_under_a_tag_we_issued() {
        // Unsigned, this is a valid resume point somewhere nobody was sent.
        let issued = signer(SECRET).encode(&scope(), &key());
        let edited = tamper(&issued, &format!("{SPOKE}|9999"));

        assert!(signer(SECRET).decode(&edited, &scope()).is_err());
    }

    #[test]
    fn refuses_a_cursor_signed_with_a_different_key() {
        let forged = signer("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").encode(&scope(), &key());

        assert!(signer(SECRET).decode(&forged, &scope()).is_err());
    }

    #[test]
    fn refuses_a_cursor_replayed_against_another_wallets_listing() {
        // The hole a bare signature leaves: genuinely ours, wrong listing.
        let issued = signer(SECRET).encode(&scope(), &key());
        let bob = Scope {
            user: address(BOB),
            ..scope()
        };

        assert!(signer(SECRET).decode(&issued, &bob).is_err());
        assert_eq!(
            signer(SECRET).decode(&issued, &scope()).ok(),
            Some(key()),
            "and still valid for the listing it was issued for"
        );
    }

    #[test]
    fn refuses_an_all_spokes_cursor_on_a_single_spoke_listing_and_the_reverse() {
        // Both directions: the sentinel only has to be wrong one way for this
        // to pass by accident, and nothing downstream would notice.
        let signer = signer(SECRET);
        let broad = signer.encode(&all_spokes(), &key());
        let narrow = signer.encode(&scope(), &key());

        assert!(signer.decode(&broad, &scope()).is_err());
        assert!(signer.decode(&narrow, &all_spokes()).is_err());
    }

    #[test]
    fn refuses_a_cursor_from_another_chain_or_another_spoke() {
        let signer = signer(SECRET);

        for elsewhere in [
            Scope {
                chain_id: 8453,
                ..scope()
            },
            Scope {
                spoke: Some(address(OTHER_SPOKE)),
                ..scope()
            },
        ] {
            let encoded = signer.encode(&elsewhere, &key());

            assert!(signer.decode(&encoded, &scope()).is_err(), "{encoded}");
        }
    }

    #[test]
    fn refuses_what_is_not_shaped_like_a_cursor_at_all() {
        for encoded in ["aGVsbG8", "a.b.c", "oh hello.and again", ""] {
            assert!(
                signer(SECRET).decode(encoded, &scope()).is_err(),
                "{encoded:?}"
            );
        }
    }

    #[test]
    fn refuses_a_payload_that_is_correctly_signed_and_still_not_a_key() {
        // Reachable only through a bug here, since a caller cannot sign one.
        let signer = signer(SECRET);

        for payload in [
            format!("{SPOKE}|13; DROP"),
            format!("{SPOKE}|"),
            format!("{SPOKE}|13|extra"),
            "|13".to_owned(),
            format!("{SPOKE}13"),
        ] {
            let signed = signer.sign(&scope(), &payload);

            assert!(signer.decode(&signed, &scope()).is_err(), "{payload:?}");
        }
    }

    #[test]
    fn takes_a_checksummed_spoke_that_we_signed_because_the_type_is_the_check() {
        // Where this parts company with the lower-case regex it replaces:
        // `Address` is case-insensitive, so the same twenty bytes come back.
        let signer = signer(SECRET);
        let signed = signer.sign(&scope(), &format!("{}|13", SPOKE.to_uppercase()));

        assert_eq!(signer.decode(&signed, &scope()).ok(), Some(key()));
    }

    #[test]
    fn says_which_of_the_seven_things_was_wrong() {
        // What a caller is told is the route's to decide; what happened is this
        // module's, and a refusal that only said "no" would be untestable here.
        let issued = signer(SECRET).encode(&scope(), &key());

        assert_eq!(refusal(&issued, &all_spokes()), Invalid::Signature);
        assert_eq!(refusal("not-one-of-ours", &scope()), Invalid::Shape);
        assert_eq!(refusal("oh hello.and again", &scope()), Invalid::Payload);
        assert_eq!(
            Invalid::Signature.to_string(),
            "signature does not match this listing"
        );
    }
}
