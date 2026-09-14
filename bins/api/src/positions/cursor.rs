//! The cursor as this API publishes it.
//!
//! **Signing lives here, not in `aave-positions`.** That crate deals in
//! [`PositionKey`] — keyset paging is how the database resumes a scan, and it is
//! the same page key whether a reconciliation asks for it or an HTTP request
//! does. Making that key opaque and unforgeable is a property of *publishing*
//! it: it exists because this service hands the key to someone it does not trust
//! and takes it back again. Nothing inside the fold ever holds a signed cursor,
//! and the key that signs one is this service's configuration.
//!
//! **What signing defends against.** Position data is public on chain, so this
//! is neither confidentiality nor access control — a caller can already ask for
//! any wallet. It buys two other things. The cursor becomes a genuinely opaque
//! contract, so its encoding can change without breaking anyone who hand-rolled
//! one. And it stops a cursor being carried between listings: unsigned, a resume
//! point from one wallet is a well-formed resume point in another's, silently
//! skipping every reserve below it.
//!
//! That second one is why the scope is **mixed into the signature rather than
//! stored in the cursor**. A tag over `(chain_id, user, spoke-filter)` plus the
//! key only verifies when the caller presents the same three, so switching
//! listings fails the same check as tampering — and the cursor stays one field
//! long.
//!
//! HMAC rather than a JWT: a JWT would add a header, algorithm negotiation and
//! the `alg: none` footgun to protect two short strings.
//!
//! **The key must be identical across replicas.** Each process signs with its
//! own copy, so a per-process key means a cursor issued by one pod is rejected
//! by the next — pagination that fails only under load, and only sometimes. The
//! same applies across implementations: the construction below is what the
//! TypeScript service also computes, so while both run, a cursor issued by
//! either is accepted by the other.

use aave_positions::store::PositionKey;
use alloy_primitives::{Address, U256};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::digest::InvalidLength;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq as _;

use crate::errors::{self, BoxedAppError};

type Keyed = Hmac<Sha256>;

/// The one separator, between every field of both the scope and the payload.
///
/// One rather than a hierarchy of them, because what makes a concatenation
/// unambiguous is not picking a rare character — it is that the separator cannot
/// appear in the values it separates. Every field here is an address, a decimal
/// id or [`ALL_SPOKES`]: none can contain this, so `a|b|c` has exactly one
/// reading. Were that not true, a tag issued for one listing would verify
/// against another with the boundary moved, and no choice of character would be
/// safe — only less obviously unsafe.
const SEP: char = '|';

/// Not a valid address, so it cannot collide with a Spoke genuinely filtered on.
const ALL_SPOKES: &str = "*";

/// 128 bits. Full SHA-256 would triple the cursor to no benefit.
const TAG_BYTES: usize = 16;

/// A key shorter than this is guessable, and a guessable key is not a signature.
///
/// Enforced in `config`, where the variable is read, rather than here: a value
/// is checked once, where it enters.
pub(crate) const MIN_SECRET_BYTES: usize = 32;

/// The listing a page belongs to: one wallet, on one chain, and either one Spoke
/// or all of them.
///
/// **`spoke` is the filter that was applied, not the Spoke a row came from.**
/// `None` means the listing spanned every Spoke. The distinction is the whole
/// point: an all-Spokes cursor is a well-formed resume point inside a
/// single-Spoke listing, so if the two scopes signed identically a caller could
/// carry one across and silently skip every reserve below it.
///
/// Not carried inside the cursor — mixed into its signature instead, so a tag
/// only verifies against the listing it was issued for.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Scope {
    pub(crate) chain_id: u32,
    pub(crate) user: Address,
    pub(crate) spoke: Option<Address>,
}

/// Signs and verifies page cursors for this deployment.
pub(crate) struct Cursors {
    /// Keyed once at boot and cloned per use. `Hmac` is `Clone`, and the key
    /// schedule is the half worth not repeating.
    keyed: Keyed,
}

impl Cursors {
    /// # Errors
    ///
    /// [`InvalidLength`], which HMAC never produces: RFC 2104 hashes a key
    /// longer than the block size and pads a shorter one, so every length is
    /// accepted. `new_from_slice` is fallible because the trait serves ciphers
    /// with a fixed key size too. Propagated rather than unwrapped, because an
    /// unconstructible arm is still not ours to panic on.
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

    /// Verifies before it parses — unauthenticated input is not worth decoding
    /// into a key a query will be run with.
    ///
    /// # Errors
    ///
    /// A 400 in every case. It is the caller's input being wrong, not this
    /// service failing, and it is raised at the point of detection rather than
    /// translated afterwards — so a genuine fault in here still surfaces as the
    /// 500 it is, with no catch-all to swallow it.
    pub(crate) fn decode(
        &self,
        encoded: &str,
        scope: &Scope,
    ) -> Result<PositionKey, BoxedAppError> {
        let (body, tag) = encoded
            .split_once('.')
            .ok_or_else(|| invalid("expected a payload and a tag"))?;
        if tag.contains('.') {
            return Err(invalid("expected a payload and a tag"));
        }

        let payload = URL_SAFE_NO_PAD
            .decode(body)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or_else(|| invalid("the payload is not base64url text"))?;

        // Constant time, and `subtle`'s slice impl answers `false` for a length
        // mismatch rather than panicking on one. A plain `==` would return on
        // the first differing byte, which leaks how much of a guessed tag was
        // right.
        if !bool::from(self.tag(scope, &payload).as_bytes().ct_eq(tag.as_bytes())) {
            return Err(invalid("signature does not match this listing"));
        }

        let (spoke, reserve_id) = payload
            .split_once(SEP)
            .ok_or_else(|| invalid("expected a Spoke and a reserve id"))?;

        Ok(PositionKey {
            spoke: spoke
                .parse()
                .map_err(|_| invalid("the Spoke is not an address"))?,
            reserve_id: decimal(reserve_id)?,
        })
    }

    /// Signs a payload this service would never build, so a case can prove what
    /// happens to one that is correctly signed and still wrong. Goes through the
    /// real [`Cursors::tag`], which is the point of it.
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

/// The resume point is `(spoke, reserve_id)`, because the scope pins everything
/// above it: within one `(chain, user)` the table's sorting key has only those
/// two left. When the listing is already narrowed to one Spoke the first half is
/// constant, but it is still signed — that is what stops the two listings
/// sharing a cursor.
///
/// Lower-case hex, which is both the spelling the fold stores and the one the
/// wire carries; `Address`'s own `Display` is EIP-55 checksummed and would sign
/// a different string for the same address.
fn payload(key: &PositionKey) -> String {
    format!("{:#x}{SEP}{}", key.spoke, key.reserve_id)
}

/// The digits of a reserve id, and only digits.
///
/// **`U256::from_str` is not this check.** Measured: it answers `Ok(0)` for the
/// empty string and reads a `0x` prefix as hex, so an id that went missing would
/// come back as the resume point at the very start of the listing rather than as
/// a refusal — a page silently served from the beginning. The payload is signed,
/// so only a bug on this side could put either there, which is exactly the bug
/// worth failing loudly on.
fn decimal(value: &str) -> Result<U256, BoxedAppError> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid("the reserve id is not a number"));
    }

    U256::from_str_radix(value, 10).map_err(|_| invalid("the reserve id does not fit uint256"))
}

/// Named, so every refusal reads the same and none of them is a 500.
fn invalid(reason: &str) -> BoxedAppError {
    errors::bad_request(format!("invalid page cursor: {reason}"))
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;

    use super::*;
    use crate::test_support::answered;

    const SECRET: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    const ALICE: &str = "0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8";
    const BOB: &str = "0xb8516f75dcf450b5b455b5114f5a92f6abd37dca";
    const SPOKE: &str = "0x94e7a5dcbe816e498b89ab752661904e2f56c485";
    const OTHER_SPOKE: &str = "0x973a023a77420ba610f06b3858ad991df6d85a08";

    fn address(hex: &str) -> Address {
        hex.parse().expect("a literal address")
    }

    fn cursors(secret: &str) -> Cursors {
        Cursors::new(secret).expect("HMAC takes a key of any length")
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

    fn refusal(encoded: &str, scope: &Scope) -> BoxedAppError {
        cursors(SECRET)
            .decode(encoded, scope)
            .expect_err("expected a refusal")
    }

    #[test]
    fn round_trips_the_resume_point_it_was_built_from() {
        let cursors = cursors(SECRET);
        let encoded = cursors.encode(&scope(), &key());

        assert_eq!(cursors.decode(&encoded, &scope()).ok(), Some(key()));
    }

    #[test]
    fn carries_the_spoke_so_an_all_spokes_walk_knows_where_it_stopped() {
        // With `spoke` unpinned it is half of what the sorting key leaves free,
        // so a resume point without it would restart at whichever Spoke sorts
        // first.
        let cursors = cursors(SECRET);
        let key = PositionKey {
            spoke: address(OTHER_SPOKE),
            reserve_id: U256::from(3),
        };
        let encoded = cursors.encode(&all_spokes(), &key);

        assert_eq!(cursors.decode(&encoded, &all_spokes()).ok(), Some(key));
    }

    #[test]
    fn stays_url_safe_so_it_needs_no_escaping_in_a_query_string() {
        let encoded = cursors(SECRET).encode(&scope(), &key());

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
        // The whole point: without a signature this is a valid resume point, and
        // the caller has silently moved themselves somewhere they were not sent.
        let issued = cursors(SECRET).encode(&scope(), &key());
        let edited = tamper(&issued, &format!("{SPOKE}|9999"));

        assert!(cursors(SECRET).decode(&edited, &scope()).is_err());
    }

    #[test]
    fn refuses_a_cursor_signed_with_a_different_key() {
        let forged = cursors("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").encode(&scope(), &key());

        assert!(cursors(SECRET).decode(&forged, &scope()).is_err());
    }

    #[test]
    fn refuses_a_cursor_replayed_against_another_wallets_listing() {
        // The correctness hole a bare signature would leave open. The key is
        // well-formed and genuinely ours — it just names a resume point in a
        // different listing, so the scope goes into the tag rather than beside
        // it.
        let issued = cursors(SECRET).encode(&scope(), &key());
        let bob = Scope {
            user: address(BOB),
            ..scope()
        };

        assert!(cursors(SECRET).decode(&issued, &bob).is_err());
        assert_eq!(
            cursors(SECRET).decode(&issued, &scope()).ok(),
            Some(key()),
            "and still valid for the listing it was issued for"
        );
    }

    #[test]
    fn refuses_an_all_spokes_cursor_on_a_single_spoke_listing_and_the_reverse() {
        // Both directions, because the sentinel only has to be wrong one way for
        // this to pass by accident. An all-Spokes resume point is *well-formed*
        // inside the narrowed listing — it names a Spoke and a reserve — so
        // nothing downstream would notice it skipping every reserve below it.
        let cursors = cursors(SECRET);
        let broad = cursors.encode(&all_spokes(), &key());
        let narrow = cursors.encode(&scope(), &key());

        assert!(cursors.decode(&broad, &scope()).is_err());
        assert!(cursors.decode(&narrow, &all_spokes()).is_err());
    }

    #[test]
    fn refuses_a_cursor_from_another_chain_or_another_spoke() {
        let cursors = cursors(SECRET);

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
            let encoded = cursors.encode(&elsewhere, &key());

            assert!(cursors.decode(&encoded, &scope()).is_err(), "{encoded}");
        }
    }

    #[test]
    fn refuses_what_is_not_shaped_like_a_cursor_at_all() {
        for encoded in ["aGVsbG8", "a.b.c", "oh hello.and again", ""] {
            assert!(
                cursors(SECRET).decode(encoded, &scope()).is_err(),
                "{encoded:?}"
            );
        }
    }

    #[test]
    fn refuses_a_payload_that_is_correctly_signed_and_still_not_a_key() {
        // Reachable only through a bug on our side, since a caller cannot
        // produce a valid tag. It fails by name here rather than as a parse
        // error from inside a query.
        let cursors = cursors(SECRET);

        for payload in [
            format!("{SPOKE}|13; DROP"),
            format!("{SPOKE}|"),
            format!("{SPOKE}|13|extra"),
            "|13".to_owned(),
            format!("{SPOKE}13"),
        ] {
            let signed = cursors.sign(&scope(), &payload);

            assert!(cursors.decode(&signed, &scope()).is_err(), "{payload:?}");
        }
    }

    #[test]
    fn takes_a_checksummed_spoke_that_we_signed_because_the_type_is_the_check() {
        // The one place this parts company with the service it replaces, which
        // matched the payload against a lower-case regex. `Address` is
        // case-insensitive by construction, so the distinction has nothing left
        // to protect: the same twenty bytes come back either way, and only our
        // own bug could put the other spelling there.
        let cursors = cursors(SECRET);
        let signed = cursors.sign(&scope(), &format!("{}|13", SPOKE.to_uppercase()));

        assert_eq!(cursors.decode(&signed, &scope()).ok(), Some(key()));
    }

    #[tokio::test]
    async fn is_a_400_that_says_which_listing_refused_it() {
        // A forged cursor is the caller's input being wrong. A 500 here would be
        // a page an operator gets woken for, over a query parameter.
        let issued = cursors(SECRET).encode(&scope(), &key());
        let (status, body) = answered(refusal(&issued, &all_spokes())).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            r#"{"message":"invalid page cursor: signature does not match this listing","error":"Bad Request","status_code":400}"#
        );
    }

    #[tokio::test]
    async fn is_a_400_for_something_that_was_never_a_cursor() {
        let (status, _) = answered(refusal("not-one-of-ours", &scope())).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
