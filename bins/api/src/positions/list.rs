//! `GET /{prefix}/v1/chains/{chain_id}/users/{user}/positions`.
//!
//! Thin, because the fold is materialized views: nothing to compute. What it
//! owns is what the store does not — the listing a cursor belongs to, and that a
//! chain with no cursor row is a missing resource rather than an empty list.
//!
//! **It is where the two enrichments are merged in**, keyed by chain rather than
//! by page and joined here rather than in SQL, which is what keeps the ClickHouse
//! adapter unaware either exists. What a position then looks like is [`super::wire`]'s.
//!

use aave_positions::store::PositionQuery;
use axum::extract::{Path, RawQuery, State};
use axum::routing::get;
use axum::{Json, Router};
use time::OffsetDateTime;

use super::cursor::Scope;
use super::params::Listing;
use super::wire::{self, Prices};
use crate::app::AppState;
use crate::errors::{self, BoxedAppError};

pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/chains/{chain_id}/users/{user}/positions", get(list))
}

/// **The query arrives raw rather than deserialized**: neither an unrecognised
/// key nor every fault at once survives a `Deserialize`.
/// # Errors
///
/// A 404 when this deployment has never indexed the chain, a 400 for a cursor
/// that does not verify against this listing, and a 500 for anything the stores
/// or the arithmetic refuse.
async fn list(
    State(app): State<AppState>,
    Path((chain_id, user)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Json<wire::Page>, BoxedAppError> {
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

    let items = page
        .items
        .iter()
        .map(|position| wire::item(position, &labels, &prices))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Json(wire::Page {
        sync: wire::Progress {
            last_block: sync.last_block,
            last_block_hash: sync.last_hash.to_string(),
            updated_at: sync.updated_at,
            age_seconds: sync.age_seconds,
            stale: sync.age_seconds > app.staleness.sync,
        },
        valued_at: OffsetDateTime::from_unix_timestamp(i64::try_from(page.valued_at)?)?,
        pricing: wire::pricing(&page.items, &prices, app.staleness.price),
        items,
        next_cursor: page.next.map(|key| app.signer.encode(&scope, &key)),
    }))
}

#[cfg(test)]
mod tests;
