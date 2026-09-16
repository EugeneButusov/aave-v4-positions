//! The route end to end, with the four ports doubled.
//!
//! One case asserts a whole body and the rest assert one decision each: the
//! key spellings, the field order and the timestamp format are one contract
//! and belong in one literal.

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

use crate::positions::params::{DEFAULT_LIMIT, GENESIS, MAX_AS_OF, MAX_LIMIT};
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
    // The one literal: key spellings, field order and both timestamps.
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
    // `200` with nothing in it says the wallet holds no positions.
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
    // Amounts extrapolate to that instant and prices do not. The counter
    // is what makes it a skip: a discarded result reads `1`.
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
    // Only diverges when the oracle refused a reserve and its last good
    // price was left to age, which is the case worth surfacing.
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
async fn writes_a_subsecond_at_whatever_width_it_takes_to_say_it() {
    // Postgres holds microseconds, and the wire publishes them rather than a
    // fixed three digits. A half second is one digit and stays one.
    let (key, mut price) = priced(SPOKE, 41);
    price.priced_at += time::Duration::milliseconds(500);

    let stores = Stores {
        page: PositionPage {
            items: vec![held()],
            valued_at: VALUED_AT,
            next: None,
        },
        labels: labelled(),
        prices: HashMap::from([(key, price)]),
        ..Stores::default()
    };

    let (_, body) = answer(stores, "").await;

    assert!(
        body.contains(r#""updated_at":"2026-07-25T17:19:19.5Z""#),
        "{body}"
    );
}

#[tokio::test]
async fn an_unresolved_reserve_nulls_the_asset_the_value_and_the_shares_together() {
    // The scale lives on the asset: an unscaled integer in a field the
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

    // A ray's scale is the protocol's fixed 27, so it survives.
    assert!(
        body.contains(r#""premium_offset_ray":"-0.000000000000000000000000001""#),
        "{body}"
    );
}

#[tokio::test]
async fn a_hub_that_listed_the_asset_without_checkpointing_it_keeps_the_shares() {
    // The other half of the rule above: `value` nulls without `asset` doing
    // so, because a zero there could not be told from a real zero balance.
    // The page's clock does not age for it either — a position with nothing
    // to value takes no price.
    let mut unvalued = held();
    unvalued.value = None;

    let stores = Stores {
        page: PositionPage {
            items: vec![unvalued],
            valued_at: VALUED_AT,
            next: None,
        },
        labels: labelled(),
        prices: HashMap::from([priced(SPOKE, 41)]),
        ..Stores::default()
    };

    let (status, body) = answer(stores, "").await;

    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#""supplied_shares":"500.5""#), "{body}");
    assert!(body.contains(r#""symbol":"USDC""#), "{body}");
    assert!(body.contains(r#""value":null"#), "{body}");
    assert!(body.contains(r#""pricing":null"#), "{body}");
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
async fn a_token_with_no_label_and_one_with_no_symbol_both_serve_null() {
    // A row the sweep has not written and a row it wrote without a symbol are
    // two states in the store and one on the wire.
    for labels in [
        HashMap::new(),
        HashMap::from([(
            address(USDC),
            TokenLabel {
                symbol: None,
                name: None,
            },
        )]),
    ] {
        let stores = Stores {
            page: PositionPage {
                items: vec![held()],
                valued_at: VALUED_AT,
                next: None,
            },
            labels,
            ..Stores::default()
        };

        let (_, body) = answer(stores, "").await;

        assert!(body.contains(r#""symbol":null,"name":null"#), "{body}");
    }
}

#[tokio::test]
async fn takes_back_the_cursor_it_issued_and_only_for_the_listing_it_issued_it_for() {
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

    let (resumed, _) = answer(Stores::default(), &format!("?cursor={cursor}")).await;

    assert_eq!(resumed, StatusCode::OK);

    // Well-formed, genuinely ours, and a listing it was not issued for.
    let (status, body) = answer(
        Stores::default(),
        &format!("?cursor={cursor}&spoke={SPOKE}"),
    )
    .await;

    // The whole body, because the demotion to a 400 and the `invalid page
    // cursor:` prefix are this layer's — `cursor` only says `Signature`.
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        body,
        concat!(
            r#"{"message":"invalid page cursor: signature does not match this listing","#,
            r#""error":"Bad Request","status_code":400}"#
        )
    );
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

#[tokio::test]
async fn documents_the_bounds_the_parser_enforces() {
    // The four numbers are written twice — once where `params` enforces them and
    // once where `#[utoipa::path]` publishes them, because utoipa's attribute
    // parser takes a literal and not a `const`. This is what keeps them equal.
    //
    // The default is the one bound with no key of its own: `default` is an
    // `IntoParams` feature and the inline form has no equivalent, so it lives in
    // the parameter's prose and is checked there.
    let request = Request::builder()
        .uri("/docs/openapi.json")
        .body(Body::empty())
        .expect("a well-formed request");

    let (_, body, _) = get(Stores::default().handler(), request).await;
    let document: serde_json::Value = serde_json::from_str(&body).expect("the document is JSON");

    let parameters =
        document["paths"]["/api/v1/chains/{chain_id}/users/{user}/positions"]["get"]["parameters"]
            .as_array()
            .expect("the operation's parameters")
            .clone();

    let parameter = |name: &str| {
        parameters
            .iter()
            .find(|parameter| parameter["name"] == name)
            .unwrap_or_else(|| panic!("no {name} parameter"))
            .clone()
    };

    assert_eq!(parameter("limit")["schema"]["minimum"], 1);
    assert_eq!(parameter("limit")["schema"]["maximum"], MAX_LIMIT);
    assert_eq!(parameter("as_of")["schema"]["minimum"], GENESIS);
    assert_eq!(parameter("as_of")["schema"]["maximum"], MAX_AS_OF);

    let prose = parameter("limit")["description"].to_string();
    assert!(
        prose.contains(&DEFAULT_LIMIT.to_string()),
        "the default went missing from the one place it is published: {prose}"
    );
}
