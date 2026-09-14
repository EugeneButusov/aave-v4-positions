//! One wallet's positions, on one chain.
//!
//! `GET /{prefix}/v1/chains/{chain_id}/users/{user}/positions`, and the whole of
//! what this service answers beyond its probes. Ordered by Spoke then reserve,
//! paged by keyset.
//!
//! The split follows what each part answers to. [`params`] is what a caller may
//! ask for, [`cursor`] is how a resume point is published and taken back,
//! [`dto`] is the shape that goes out and [`scale`] the one decision every
//! number in it shares. [`page`] is the join: four reads, two of them
//! enrichment, and the three clocks they carry.
//!
//! **Positions from different Spokes may be listed together but never summed.**
//! Each Spoke is an isolated margin account with its own collateral factors,
//! oracle and health factor (§12.3), so a total across two of them hides an
//! imminent liquidation behind unrelated collateral. Every row names the Spoke
//! it came from, and nothing here aggregates.

mod cursor;
mod dto;
mod page;
mod params;
mod scale;

use axum::extract::{Path, RawQuery, State};
use axum::routing::get;
use axum::{Json, Router};

pub(crate) use cursor::{Cursors, MIN_SECRET_BYTES};

use crate::app::AppState;
use crate::errors::BoxedAppError;
use params::Listing;

/// The paths this module serves, relative to the version prefix `router` mounts
/// them under.
pub(crate) fn routes() -> Router<AppState> {
    Router::new().route("/chains/{chain_id}/users/{user}/positions", get(list))
}

/// **The query arrives raw rather than deserialized**, because two of the three
/// things this endpoint has to say about it do not survive a `Deserialize`: an
/// unrecognised key, and every fault at once rather than the first.
async fn list(
    State(app): State<AppState>,
    Path((chain_id, user)): Path<(String, String)>,
    RawQuery(query): RawQuery,
) -> Result<Json<dto::Page>, BoxedAppError> {
    let listing = Listing::parse(&chain_id, &user, query.as_deref().unwrap_or_default())?;

    page::build(&app, &listing).await.map(Json)
}

#[cfg(test)]
mod tests {
    //! The route end to end, with the four ports doubled.
    //!
    //! One case asserts a whole body and the rest assert one decision each. That
    //! split is deliberate: the key spellings, the field order and the timestamp
    //! format are one contract and belong in one literal, where a change to any
    //! of them shows as a change to this file rather than as six near-identical
    //! diffs.

    use std::collections::HashMap;
    use std::sync::atomic::Ordering;

    use aave_positions::store::{Position, PositionAsset, PositionKey, PositionPage};
    use aave_positions::valuation::Valuation;
    use alloy_primitives::{Address, I256, U256, uint};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use indexing::SyncStatus;
    use prices::{ReserveKey, ReservePrice};
    use time::OffsetDateTime;
    use token_metadata::TokenLabel;

    use crate::test_support::{Stores, VALUED_AT, get, synced};

    const ALICE: &str = "0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8";
    const SPOKE: &str = "0x94e7a5dcbe816e498b89ab752661904e2f56c485";
    const OTHER_SPOKE: &str = "0x973a023a77420ba610f06b3858ad991df6d85a08";
    const HUB: &str = "0xcca852bc40e560adc3b1cc58ca5b55638ce826c9";
    const USDC: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";

    const RAY: U256 = uint!(1_000_000_000_000_000_000_000_000_000_U256);

    fn address(hex: &str) -> Address {
        hex.parse().expect("a literal address")
    }

    /// 1000 USDC supplied, nothing borrowed, on the Main Spoke's reserve 7.
    fn held() -> Position {
        Position {
            chain_id: 1,
            user: address(ALICE),
            spoke: address(SPOKE),
            reserve_id: U256::from(7),
            supplied_shares: I256::unchecked_from(500_500_000),
            drawn_shares: I256::ZERO,
            premium_shares: I256::ZERO,
            premium_offset_ray: I256::ZERO,
            net_supplied_amount: I256::unchecked_from(500_500_000),
            net_borrowed_amount: I256::ZERO,
            using_as_collateral: true,
            events: 3,
            asset: Some(PositionAsset {
                asset_id: U256::from(7),
                hub: address(HUB),
                underlying: address(USDC),
                decimals: 6,
            }),
            value: Some(Valuation {
                supplied_amount: U256::from(1_000_000_000u64),
                drawn_debt: U256::ZERO,
                premium_debt: U256::ZERO,
                total_debt: U256::ZERO,
                drawn_index: RAY,
            }),
        }
    }

    /// $0.99971505, as §7.4.3 records it, read 41 seconds ago.
    fn priced(spoke: &str, age_seconds: u64) -> (ReserveKey, ReservePrice) {
        (
            ReserveKey {
                spoke: address(spoke),
                reserve_id: U256::from(7),
            },
            ReservePrice {
                price: U256::from(99_971_505u64),
                priced_at: OffsetDateTime::from_unix_timestamp(
                    i64::try_from(VALUED_AT - age_seconds).expect("a representable instant"),
                )
                .expect("a representable instant"),
                age_seconds,
            },
        )
    }

    fn labelled() -> HashMap<Address, TokenLabel> {
        HashMap::from([(
            address(USDC),
            TokenLabel {
                symbol: Some("USDC".to_owned()),
                name: Some("USD Coin".to_owned()),
            },
        )])
    }

    fn request(query: &str) -> Request<Body> {
        Request::builder()
            .uri(format!("/api/v1/chains/1/users/{ALICE}/positions{query}"))
            .body(Body::empty())
            .expect("a well-formed request")
    }

    async fn answer(stores: Stores, query: &str) -> (StatusCode, String) {
        let (status, body, _) = get(stores.handler(), request(query)).await;

        (status, body)
    }

    #[tokio::test]
    async fn serves_the_whole_page_in_the_spelling_this_port_publishes() {
        // The one literal. Every multi-word key is snake_case where the service
        // beside this one writes camelCase, both timestamps are RFC 3339 with no
        // invented milliseconds, and the field order is the order the contract
        // declares.
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: None,
            },
            labels: labelled(),
            prices: HashMap::from([priced(SPOKE, 41)]),
            ..Stores::default()
        };

        let (status, body) = answer(stores, "").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            concat!(
                r#"{"sync":{"last_block":25652535,"#,
                r#""last_block_hash":"0xabababababababababababababababababababababababababababababababab","#,
                r#""updated_at":"2026-07-25T17:21:11Z","age_seconds":7,"stale":false},"#,
                r#""valued_at":"2026-07-25T17:20:00Z","#,
                r#""pricing":{"updated_at":"2026-07-25T17:19:19Z","age_seconds":41,"stale":false},"#,
                r#""items":[{"chain_id":1,"#,
                r#""user":"0x82d16ff1c724ab72f218a3f7f6dd3e5385ee87e8","#,
                r#""spoke":"0x94e7a5dcbe816e498b89ab752661904e2f56c485","#,
                r#""reserve_id":"7","supplied_shares":"500.5","drawn_shares":"0","#,
                r#""premium_shares":"0","premium_offset_ray":"0","#,
                r#""net_supplied_amount":"500.5","net_borrowed_amount":"0","#,
                r#""using_as_collateral":true,"events":3,"#,
                r#""asset":{"asset_id":"7","#,
                r#""hub":"0xcca852bc40e560adc3b1cc58ca5b55638ce826c9","#,
                r#""underlying":"0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48","#,
                r#""decimals":6,"symbol":"USDC","name":"USD Coin"},"#,
                r#""value":{"supplied_amount":"1000","drawn_debt":"0","premium_debt":"0","#,
                r#""total_debt":"0","drawn_index":"1","price_usd":"0.99971505","#,
                r#""supplied_amount_usd":"999.71505","total_debt_usd":"0"}}],"#,
                r#""next_cursor":null}"#,
            )
        );
    }

    #[tokio::test]
    async fn a_chain_this_deployment_has_never_indexed_is_a_404_not_an_empty_page() {
        // Answering `200` with nothing in it would say the wallet holds no
        // positions, which is a different claim and a wrong one.
        let (status, body) = answer(
            Stores {
                sync: None,
                ..Stores::default()
            },
            "",
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(
            body,
            r#"{"message":"chain 1 has not been indexed by this deployment","error":"Not Found","status_code":404}"#
        );
    }

    #[tokio::test]
    async fn an_explicit_instant_skips_the_price_read_rather_than_discarding_it() {
        // Amounts are extrapolated to that instant and prices are not, so a
        // value mixing the two would be a number that never existed. The
        // counter is what makes this a skip: a discarded result reads `1`.
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: 1_780_000_000,
                next: None,
            },
            labels: labelled(),
            prices: HashMap::from([priced(SPOKE, 41)]),
            ..Stores::default()
        };
        let reads = stores.price_reads.clone();

        let (status, body) = answer(stores, "?as_of=1780000000").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(reads.load(Ordering::Relaxed), 0);
        assert!(body.contains(r#""pricing":null"#), "{body}");
        assert!(body.contains(r#""price_usd":null"#), "{body}");
        assert!(body.contains(r#""supplied_amount_usd":null"#), "{body}");
        assert!(body.contains(r#""total_debt_usd":null"#), "{body}");
        assert!(
            body.contains(r#""valued_at":"2026-05-28T20:26:40Z""#),
            "{body}"
        );
    }

    #[tokio::test]
    async fn the_page_clock_is_the_oldest_price_behind_it_not_the_newest() {
        // Prices are normally written in one upsert and share a timestamp, so
        // this only diverges when the oracle refused a reserve and its last good
        // price was left to age — which is exactly the case worth surfacing.
        let mut elsewhere = held();
        elsewhere.spoke = address(OTHER_SPOKE);

        let stores = Stores {
            page: PositionPage {
                items: vec![held(), elsewhere],
                valued_at: VALUED_AT,
                next: None,
            },
            labels: labelled(),
            prices: HashMap::from([priced(SPOKE, 41), priced(OTHER_SPOKE, 3_600)]),
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;

        assert!(
            body.contains(
                r#""pricing":{"updated_at":"2026-07-25T16:20:00Z","age_seconds":3600,"stale":true}"#
            ),
            "{body}"
        );
    }

    #[tokio::test]
    async fn an_unresolved_reserve_nulls_the_asset_the_value_and_the_shares_together() {
        // The scale lives on the asset, so without it there is no honest way to
        // render a share balance — and an unscaled integer in a field the
        // contract calls decimal is wrong by up to eighteen orders of magnitude.
        let mut unresolved = held();
        unresolved.asset = None;
        unresolved.value = None;
        unresolved.premium_offset_ray = I256::unchecked_from(-1);

        let stores = Stores {
            page: PositionPage {
                items: vec![unresolved],
                valued_at: VALUED_AT,
                next: None,
            },
            ..Stores::default()
        };

        let (status, body) = answer(stores, "").await;

        assert_eq!(status, StatusCode::OK);
        for null in [
            r#""supplied_shares":null"#,
            r#""drawn_shares":null"#,
            r#""premium_shares":null"#,
            r#""net_supplied_amount":null"#,
            r#""net_borrowed_amount":null"#,
            r#""asset":null"#,
            r#""value":null"#,
            r#""pricing":null"#,
        ] {
            assert!(body.contains(null), "{null} missing from {body}");
        }

        // A ray is a ratio, so its scale is the protocol's fixed 27 rather than
        // the asset's, and it survives what nulls everything beside it.
        assert!(
            body.contains(r#""premium_offset_ray":"-0.000000000000000000000000001""#),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_reserve_nobody_has_priced_nulls_the_dollars_and_keeps_the_tokens() {
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: None,
            },
            labels: labelled(),
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;

        assert!(body.contains(r#""supplied_amount":"1000""#), "{body}");
        assert!(body.contains(r#""price_usd":null"#), "{body}");
        assert!(body.contains(r#""pricing":null"#), "{body}");
    }

    #[tokio::test]
    async fn a_token_enrichment_has_not_reached_serves_a_null_label() {
        // Absent from the map and present with a null symbol both serve null:
        // the wire cannot express the difference and a caller has no use for it.
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: None,
            },
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;

        assert!(body.contains(r#""symbol":null,"name":null"#), "{body}");
    }

    #[tokio::test]
    async fn hands_back_a_cursor_it_will_take_again() {
        // The round trip, through the router rather than through the codec: a
        // cursor is only useful if the listing that issued it accepts it back.
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: Some(PositionKey {
                    spoke: address(SPOKE),
                    reserve_id: U256::from(7),
                }),
            },
            labels: labelled(),
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;
        let page: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
        let cursor = page["next_cursor"].as_str().expect("a cursor").to_owned();

        let (status, _) = answer(Stores::default(), &format!("?cursor={cursor}")).await;

        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn refuses_a_cursor_issued_for_a_different_listing() {
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: Some(PositionKey {
                    spoke: address(SPOKE),
                    reserve_id: U256::from(7),
                }),
            },
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;
        let page: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
        let cursor = page["next_cursor"].as_str().expect("a cursor").to_owned();

        // The same cursor, narrowed to one Spoke. Well-formed, genuinely ours,
        // and a resume point in a listing it was not issued for.
        let (status, body) = answer(
            Stores::default(),
            &format!("?cursor={cursor}&spoke={SPOKE}"),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("does not match this listing"), "{body}");
    }

    #[tokio::test]
    async fn a_bad_parameter_is_a_400_through_the_router_and_not_only_in_the_parser() {
        let handler = Stores::default().handler();
        let request = Request::builder()
            .uri("/api/v1/chains/1/users/0xnope/positions")
            .body(Body::empty())
            .expect("a well-formed request");

        let (status, body, _) = get(handler, request).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body.contains("user must be a 20-byte hex address"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn is_mounted_under_the_prefix_and_nowhere_else() {
        // The prefix is configuration, so the one thing worth pinning is that
        // the route is not also reachable without it.
        let handler = Stores::default().handler();
        let request = Request::builder()
            .uri(format!("/v1/chains/1/users/{ALICE}/positions"))
            .body(Body::empty())
            .expect("a well-formed request");

        let (status, _, _) = get(handler, request).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stamps_a_page_stale_when_the_indexer_has_stopped_advancing() {
        let stores = Stores {
            sync: Some(SyncStatus {
                age_seconds: 61,
                ..synced()
            }),
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;

        assert!(body.contains(r#""age_seconds":61,"stale":true"#), "{body}");
    }
}
