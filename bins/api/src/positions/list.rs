//! `GET /{prefix}/v1/chains/{chain_id}/users/{user}/positions`.
//!
//! Thin, because the fold is materialized views: nothing to compute. What it
//! owns is what the store does not — the listing a cursor belongs to, and that a
//! chain with no cursor row is a missing resource rather than an empty list.
//!
//! **It is where the two enrichments are merged in**, keyed by chain rather than
//! by page and joined here rather than in SQL, which is what keeps the ClickHouse
//! adapter unaware either exists. What a position then looks like is [`super::views`]'s.

use aave_positions::store::PositionQuery;
use axum::Json;
use axum::extract::{Path, RawQuery, State};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::cursor::Scope;
use super::params::Listing;
use super::views::{self, Prices};
use crate::app::AppState;
use crate::errors::{self, ApiErrorResponse, BoxedAppError};

pub(crate) fn routes() -> OpenApiRouter<AppState> {
    OpenApiRouter::new().routes(routes!(list))
}

/// What a caller is told this endpoint does, as opposed to what the doc comment
/// below tells whoever maintains it.
const DESCRIPTION: &str = "\
One wallet, on one chain, on one Spoke or on all of them. Ordered by Spoke then \
reserve, paged by keyset.

**Every number is a decimal string, already scaled.** Amounts are in whole \
tokens, `price_usd` and the `*_usd` fields are in dollars, and `drawn_index` and \
`premium_offset_ray` are ray ratios where `1` is no accrual. Nothing on the wire \
is in base units, so no caller has to know that a Value is `1e26` to the dollar \
or that a ray is `1e27` — but they are **strings**, and parsing one into a float \
undoes the point: a share balance of `422166581625087.607993` has 21 significant \
digits and a double keeps 17. Use a decimal type.

**Balances are shares, not asset amounts.** A share is not a balance: the Hub \
index accrues every second and emits nothing, so `value` carries the shares \
converted at `valued_at` and the top-level figures are what the events \
themselves said. `net_supplied_amount` is a flow — what went in less what came \
out — and the difference between it and `value.supplied_amount` is interest.

**A field is null when the answer is unknown, never zero.** `asset` and `value` \
are null together when the registry has not resolved the reserve, and the share \
fields go with them, because the decimals that scale them live on the asset. The \
`*_usd` fields are null when the oracle has not priced the reserve, and whenever \
`as_of` is set. No health factor yet; the derivation is in \
`docs/aave-v4-protocol-analysis.md`.

**Positions from different Spokes may be listed together but never summed.** \
Each Spoke is an isolated margin account with its own collateral factors, oracle \
and health factor, so a total across two of them hides an imminent liquidation \
behind unrelated collateral. Every row names the Spoke it came from.";

/// **The query arrives raw rather than deserialized**: neither an unrecognised
/// key nor every fault at once survives a `Deserialize`.
///
/// The published text is [`DESCRIPTION`] and the attribute below, not this
/// comment — utoipa would otherwise put `# Errors` in the contract.
///
/// # Errors
///
/// A 404 when this deployment has never indexed the chain, a 400 for a cursor
/// that does not verify against this listing, and a 500 for anything the stores
/// or the arithmetic refuse.
#[utoipa::path(
    get,
    path = "/chains/{chain_id}/users/{user}/positions",
    tag = "positions",
    summary = "List a wallet's positions",
    description = DESCRIPTION,
    params(
        ("chain_id" = u32, Path, description = "The chain to read. Required.", example = 1),
        (
            "user" = String,
            Path,
            description = "The position owner. Checksummed or lower-case, either matches.",
            example = "0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8",
        ),
        (
            "spoke" = Option<String>,
            Query,
            description = "Narrow to one Spoke. Omit to list every Spoke this wallet has \
                           touched.",
            example = "0x94e7a5dcbe816e498b89ab752661904e2f56c485",
        ),
        // The bounds are literals because utoipa's attribute parser takes
        // nothing else here — not a `const`, and not a `default`, which only the
        // `IntoParams` derive carries and which would cost a third declaration
        // of these four names. `documents_the_bounds_the_parser_enforces` is
        // what keeps the numbers equal to `params`'.
        (
            "limit" = Option<u32>,
            Query,
            minimum = 1,
            maximum = 200,
            description = "How many positions to return. Defaults to 50.",
        ),
        (
            "cursor" = Option<String>,
            Query,
            description = "A `next_cursor` from a previous page, handed back verbatim. Only \
                           valid for the listing that issued it.",
        ),
        (
            "as_of" = Option<u64>,
            Query,
            minimum = 1_774_277_159_u64,
            maximum = 4_102_444_800_u64,
            description = "Unix seconds to value the page at. Defaults to now — amounts \
                           accrue every second and the chain emits nothing while they do, so \
                           naming the instant is what makes a response reproducible. Echoed \
                           back as `valued_at`.",
        ),
    ),
    responses(
        (status = 200, description = "One page of this wallet's positions.", body = views::Page),
        (
            status = 400,
            description = "A malformed address, a limit outside its bounds, an unrecognised \
                           query parameter, or a cursor this service did not issue for this \
                           listing.",
            body = ApiErrorResponse<'_>,
        ),
        (
            status = 404,
            description = "This deployment has never indexed that chain, so it cannot answer \
                           for it. Also returned in the window between an indexer starting \
                           and recording its first block.",
            body = ApiErrorResponse<'_>,
        ),
    ),
)]
async fn list(
    State(app): State<AppState>,
    Path((chain_id, user)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Json<views::Page>, BoxedAppError> {
    let listing = Listing::parse(&chain_id, &user, query.as_deref().unwrap_or_default())?;

    // Read first, and fail here rather than after a query that would answer "no
    // positions" for a chain this deployment does not follow.
    let Some(sync) = app.sync.get(listing.chain_id).await? else {
        return Err(errors::not_found(format!(
            "chain {} has not been indexed by this deployment",
            listing.chain_id
        )));
    };

    let scope = Scope {
        chain_id: listing.chain_id,
        user: listing.user,
        spoke: listing.spoke,
    };

    let query = PositionQuery {
        chain_id: listing.chain_id,
        user: listing.user,
        spoke: listing.spoke,
        limit: listing.limit,
        after: listing
            .cursor
            .as_deref()
            // **The codec says what was wrong; this says what it costs.** It
            // knows nothing of statuses, so the demotion to a 400 happens here,
            // where the answer is already an HTTP one.
            .map(|cursor| {
                app.signer.decode(cursor, &scope).map_err(|invalid| {
                    errors::bad_request(format!("invalid page cursor: {invalid}"))
                })
            })
            .transpose()?,
        as_of: listing.as_of,
    };

    // Side by side: neither enrichment depends on which positions come back.
    // Measured on the service this replaces at 0.27 ms against a 28 ms page.
    let (page, labels, prices) = tokio::join!(
        app.positions.list(&query),
        app.tokens.labels(listing.chain_id),
        async {
            if listing.as_of.is_some() {
                // **An explicit `as_of` is served without prices at all**, and
                // skipped rather than discarded. Amounts extrapolate to that
                // instant; the stored price is whatever the oracle last said,
                // which is now. Pricing one against the other is a number that
                // never existed, and a price series is not what this stores.
                return Ok(Prices::new());
            }
            app.prices.latest(listing.chain_id).await
        }
    );
    let (page, labels, prices) = (page?, labels?, prices?);

    let next_cursor = page.next.as_ref().map(|key| app.signer.encode(&scope, key));

    Ok(Json(views::Page::new(
        &sync,
        &page,
        &labels,
        &prices,
        app.staleness,
        next_cursor,
    )?))
}

#[cfg(test)]
mod tests;
