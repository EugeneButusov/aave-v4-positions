//! The document's own metadata, and what belongs in it.
//!
//! **The versioned API and nothing else.** The probes are absent deliberately:
//! [`ops::probe_router`]'s own doc calls their paths infrastructure, and
//! [`crate::config`]'s calls an orchestrator's check no part of the API's
//! versioned surface. A contract that published them would contradict both.
//! `docs/rust-migration.md` records the difference from the document this
//! replaces, which carries them under a `health` tag.
//!
//! The operations are not listed here. They arrive with the routers that serve
//! them, through `utoipa_axum::routes!`, so a path cannot be documented without
//! being served or served without being documented.

use utoipa::OpenApi;

/// The version of the **API contract**, not of the package. They are
/// legitimately different things: a dependency bump changes the package and must
/// not imply anything about the shape of the responses.
///
/// `0.2.0` rather than the `0.1.0` the service this replaces publishes. Twenty-
/// four payload keys, one query parameter and the timestamp format all move, so
/// the two are not the same contract — and a version naming two shapes is the
/// one thing this field exists to prevent.
const CONTRACT_VERSION: &str = "0.2.0";

const DESCRIPTION: &str = "\
Indexed Aave v4 user positions, enriched and served for querying.

Positions in v4 have no token representation — no `aToken`, no `Transfer` \
events. They exist only as structs in Spoke storage, so everything served here \
is folded from the Spoke and Hub event streams rather than read back from chain \
state. Protocol details and the derivations behind each field are in \
`docs/aave-v4-protocol-analysis.md`.

**Integer amounts are strings.** Share balances exceed the 53-bit mantissa of a \
JSON number, and the failure mode of getting this wrong is a few wei of drift \
that reads as a rounding bug rather than a parse error.";

/// The title, the contract version and the one tag. Everything else is merged
/// in from the routers.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "Aave v4 Positions API",
        description = DESCRIPTION,
        version = CONTRACT_VERSION,
        // Written out rather than left off. `#[derive(OpenApi)]` falls back to
        // `CARGO_PKG_LICENSE`, which is empty here because the workspace members
        // are `publish = false` and carry no `license` field — so omitting this
        // publishes `"license": {"name": ""}` rather than nothing.
        license(name = "MIT"),
    ),
    tags((
        name = "positions",
        description = "Folded from the Spoke event stream, enriched from the Hub and the \
                       reserve registry. Balances are shares; `value` is what they are worth \
                       at the instant the page names. Every response carries the block it is \
                       true as of, and the two clocks beside it.",
    )),
)]
pub(crate) struct ApiDoc;
