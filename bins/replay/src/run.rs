//! Driving the corpus against both services.

use crate::compare::{self, Rules};
use crate::config::{self, Config};
use crate::corpus::{self, Body, Case, Dialect, Listing};
use crate::fold::{self, Fold};
use crate::http::{self, Answer, Client};
use crate::json::Json;
use crate::keys;
use crate::report::{Finding, Report};

/// A cursor that never empties is a bug on one side; this is what says so
/// rather than running forever. The Main Spoke lists fourteen reserves, so a
/// `limit=1` walk is fourteen pages.
const MAX_PAGES: usize = 500;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Configuration(#[from] config::Error),

    #[error("the fold would not read")]
    Fold(#[from] clickhouse_client::clickhouse::error::Error),

    #[error(transparent)]
    Unreachable(#[from] http::Error),

    #[error("chain {0} has no positions in this fold, so there is nothing to replay")]
    EmptyFold(u32),

    #[error(
        "the {which} at {base} answered {status} to /health/live, so it is not one of these services"
    )]
    NotThisService {
        which: &'static str,
        base: String,
        status: u16,
    },
}

/// # Errors
///
/// [`Error`] when a service cannot be reached or the fold cannot be read.
/// A difference between the two is a finding in the report, not an error.
pub(crate) async fn gate() -> Result<Report, Error> {
    let config = Config::from_env()?;
    let fold = fold::read(config.clickhouse.clone(), config.chain_id).await?;

    replay(&config, &fold).await
}

/// # Errors
///
/// [`Error`] when a service cannot be reached or is not one of these two.
async fn replay(config: &Config, fold: &Fold) -> Result<Report, Error> {
    let client = Client::new();
    let mut report = Report::default();

    let Some(first) = fold.wallets.first() else {
        return Err(Error::EmptyFold(config.chain_id));
    };

    for (which, base) in [("oracle", &config.oracle), ("candidate", &config.candidate)] {
        let answer = client.get(base, "GET", "/health/live").await?;

        if !answers_liveness(answer.status, &answer.body) {
            return Err(Error::NotThisService {
                which,
                base: base.clone(),
                status: answer.status,
            });
        }
    }

    for case in corpus::cases(config.chain_id, &first.user, fold) {
        one(&client, config, &case, &mut report).await?;
    }

    for listing in corpus::listings(fold, config.chain_id, config.wallets) {
        walk(&client, config, &listing, &mut report).await?;
    }

    Ok(report)
}

async fn one(
    client: &Client,
    config: &Config,
    case: &Case,
    report: &mut Report,
) -> Result<(), Error> {
    let oracle = client
        .get(&config.oracle, case.method, &case.oracle)
        .await?;
    let candidate = client
        .get(&config.candidate, case.method, &case.candidate)
        .await?;

    report.compared();
    judge(
        &case.label,
        &case.candidate,
        &case.body,
        &oracle,
        &candidate,
        report,
    );

    Ok(())
}

/// One listing to exhaustion. Each page's cursor is read off both answers and
/// has to be the same bytes before either service is resumed with it — the
/// construction is shared, so a page two that only one side can serve is a
/// finding about the signature rather than about the page.
async fn walk(
    client: &Client,
    config: &Config,
    listing: &Listing,
    report: &mut Report,
) -> Result<(), Error> {
    let body = Body::Json(listing.rules(config.skew));
    let mut cursor: Option<String> = None;

    for page in 1..=MAX_PAGES {
        let target = listing.target(Dialect::Candidate, cursor.as_deref());
        let oracle = client
            .get(
                &config.oracle,
                "GET",
                &listing.target(Dialect::Oracle, cursor.as_deref()),
            )
            .await?;
        let candidate = client.get(&config.candidate, "GET", &target).await?;

        let label = format!("{} page {page}", listing.label());
        report.compared();
        judge(&label, &target, &body, &oracle, &candidate, report);

        let shared = shared_cursor(
            next_cursor(&oracle.body, "nextCursor"),
            next_cursor(&candidate.body, "next_cursor"),
        );

        match shared {
            Err(detail) => {
                report.found(Finding {
                    label,
                    target,
                    detail: vec![detail],
                });

                return Ok(());
            }
            Ok(None) => return Ok(()),
            Ok(Some(next)) => cursor = Some(next),
        }
    }

    report.found(Finding {
        label: listing.label(),
        target: listing.target(Dialect::Candidate, None),
        detail: vec![format!("still paging after {MAX_PAGES} pages")],
    });

    Ok(())
}

fn judge(
    label: &str,
    target: &str,
    body: &Body,
    oracle: &Answer,
    candidate: &Answer,
    report: &mut Report,
) {
    let mut detail = Vec::new();

    if oracle.status != candidate.status {
        detail.push(format!(
            "status {} against {}",
            oracle.status, candidate.status
        ));
    }

    // A 5xx is compared on its status and nothing else: one service answers a
    // second error envelope there and the other answers plain text.
    if oracle.status >= 500 || candidate.status >= 500 {
        report.unasserted(label, "a 5xx is compared on its status alone");
    } else {
        match bodies(body, &oracle.body, &candidate.body) {
            Judgement::Agreed => {}
            Judgement::StatusOnly(reason) => report.unasserted(label, reason),
            Judgement::Differs(differences) => detail.extend(differences),
        }
    }

    if !detail.is_empty() {
        report.found(Finding {
            label: label.to_owned(),
            target: target.to_owned(),
            detail,
        });
    }
}

enum Judgement {
    Agreed,
    /// The status matched and the body could not be held to anything stronger.
    StatusOnly(&'static str),
    Differs(Vec<String>),
}

fn bodies(body: &Body, oracle: &str, candidate: &str) -> Judgement {
    let (left, right) = match (Json::parse(oracle), Json::parse(candidate)) {
        (Some(left), Some(right)) => (left, right),
        (None, None) => return text(oracle, candidate),
        (None, _) => return Judgement::Differs(vec![format!("the oracle served {oracle:?}")]),
        (_, None) => return Judgement::Differs(vec![format!("this service served {candidate:?}")]),
    };

    match body {
        Body::Json(rules) => read(&left, &right, rules),
        Body::Parameters => parameters(&left, &right),
    }
}

fn read(oracle: &Json, candidate: &Json, rules: &Rules) -> Judgement {
    let read = match keys::snake_case(oracle.clone()) {
        Ok(read) => read,
        Err(collision) => {
            return Judgement::Differs(vec![format!(
                "the oracle's keys will not read: {collision}"
            )]);
        }
    };

    let differences = compare::compare(&read, candidate, rules);

    if differences.is_empty() {
        Judgement::Agreed
    } else {
        Judgement::Differs(differences.iter().map(ToString::to_string).collect())
    }
}

/// A refusal's wording is each service's own. What it names is not — except
/// where the oracle names nothing, which no rule can hold the candidate to.
fn parameters(oracle: &Json, candidate: &Json) -> Judgement {
    let named = |body: &Json| {
        body.get("message")
            .and_then(Json::as_str)
            .map(compare::parameters_named)
    };

    match (named(oracle), named(candidate)) {
        (Some(theirs), Some(_)) if theirs.is_empty() => {
            Judgement::StatusOnly("the oracle's message names no parameter")
        }
        (Some(theirs), Some(ours)) if theirs.is_subset(&ours) => Judgement::Agreed,
        (Some(theirs), Some(ours)) => {
            Judgement::Differs(vec![format!("names {theirs:?} against {ours:?}")])
        }
        _ => Judgement::Differs(vec!["a refusal with no message".to_owned()]),
    }
}

/// Neither body is JSON, which is the 500's shape on one side and nothing
/// else's. Compared as bytes.
fn text(oracle: &str, candidate: &str) -> Judgement {
    if oracle == candidate {
        Judgement::Agreed
    } else {
        Judgement::Differs(vec![format!("{oracle:?} against {candidate:?}")])
    }
}

/// Whether a base URL is serving one of these two services at all.
///
/// The defaults are ports a developer machine often has something else on, and
/// a wall of findings against a service that is not the oracle is worse than a
/// refusal. Liveness is the cheapest thing both are known to answer.
fn answers_liveness(status: u16, body: &str) -> bool {
    status == 200
        && Json::parse(body)
            .as_ref()
            .and_then(|read| read.get("status"))
            .and_then(Json::as_str)
            == Some("ok")
}

/// Both services sign a resume point the same way, so one string walks both.
/// A pair that differs ends the walk rather than feeding each service its own
/// cursor, which would be comparing two different pages from there on.
fn shared_cursor(theirs: Option<String>, ours: Option<String>) -> Result<Option<String>, String> {
    if theirs == ours {
        Ok(ours)
    } else {
        Err(format!("next cursor {theirs:?} against {ours:?}"))
    }
}

fn next_cursor(body: &str, key: &str) -> Option<String> {
    Json::parse(body)?.get(key)?.as_str().map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use crate::compare::Rules;
    use crate::config::Config;
    use crate::corpus::{Body, Listing};
    use crate::fold::{Fold, Wallet};
    use crate::http::{Answer, Client};
    use crate::report::Report;
    use crate::test_support::Stub;

    use super::{
        Error, Judgement, answers_liveness, bodies, judge, next_cursor, replay, shared_cursor, walk,
    };

    const PAGE: &str = "/v1/chains/1/users/0xaa/positions";

    fn config(oracle: &Stub, candidate: &Stub) -> Config {
        Config {
            oracle: oracle.base(),
            candidate: candidate.base(),
            chain_id: 1,
            skew: 5,
            wallets: 0,
            clickhouse: clickhouse_client::Config {
                url: "http://localhost:8123".to_owned(),
                database: "aave".to_owned(),
                user: "default".to_owned(),
                password: String::new(),
            },
        }
    }

    fn fold() -> Fold {
        Fold {
            wallets: vec![Wallet {
                user: "0xaa".to_owned(),
                spokes: vec!["0x01".to_owned()],
            }],
            latest: 1_780_000_000,
        }
    }

    async fn walked(oracle: &Stub, candidate: &Stub) -> Report {
        let mut report = Report::default();
        let configured = config(oracle, candidate);

        walk(
            &Client::new(),
            &configured,
            &Listing::new(1, "0xaa", None, None, None),
            &mut report,
        )
        .await
        .unwrap();

        report
    }

    fn answer(status: u16, body: &str) -> Answer {
        Answer {
            status,
            body: body.to_owned(),
        }
    }

    fn verdict(oracle: &Answer, candidate: &Answer, body: &Body) -> Report {
        let mut report = Report::default();
        report.compared();
        judge("a case", "/target", body, oracle, candidate, &mut report);
        report
    }

    #[test]
    fn two_answers_that_agree_under_the_transform_are_no_finding() {
        let report = verdict(
            &answer(
                200,
                r#"{"nextCursor":null,"valuedAt":"2026-07-25T17:20:00.000Z"}"#,
            ),
            &answer(
                200,
                r#"{"next_cursor":null,"valued_at":"2026-07-25T17:20:00Z"}"#,
            ),
            &Body::Json(Rules::exact()),
        );

        assert!(report.agreed(), "{}", report.text());
    }

    #[test]
    fn a_status_that_differs_is_a_finding_on_its_own() {
        let report = verdict(
            &answer(200, "{}"),
            &answer(404, "{}"),
            &Body::Json(Rules::exact()),
        );

        assert!(report.text().contains("status 200 against 404"));
    }

    /// Both sides 5xx: the two shapes there are a JSON envelope and plain
    /// text, so the body is not compared and the run says so.
    #[test]
    fn a_5xx_is_held_to_its_status_alone() {
        let report = verdict(
            &answer(
                500,
                r#"{"statusCode":500,"message":"Internal server error"}"#,
            ),
            &answer(500, "Internal Server Error"),
            &Body::Json(Rules::exact()),
        );

        assert!(report.agreed(), "{}", report.text());
        assert!(report.text().contains("compared on its status alone"));
    }

    #[test]
    fn a_refusal_passes_when_the_oracles_parameters_are_among_ours() {
        let agreed = bodies(
            &Body::Parameters,
            r#"{"message":"✖ Invalid input\n  → at user"}"#,
            r#"{"message":"user must be a 20-byte hex address, got \"0xzz\"; limit must be between 1 and 200, got \"0\""}"#,
        );

        assert!(matches!(agreed, Judgement::Agreed));
    }

    /// The containment runs one way. A parameter the oracle refuses and the
    /// candidate accepts is the finding this rule exists for.
    #[test]
    fn a_refusal_fails_when_the_oracle_names_one_we_do_not() {
        let differs = bodies(
            &Body::Parameters,
            r#"{"message":"✖ Invalid input\n  → at spoke"}"#,
            r#"{"message":"limit must be between 1 and 200, got \"0\""}"#,
        );

        assert!(matches!(differs, Judgement::Differs(_)));
    }

    #[test]
    fn a_refusal_the_oracle_words_without_a_parameter_is_left_unasserted() {
        let unasserted = bodies(
            &Body::Parameters,
            r#"{"message":"Unrecognized key: \"limt\""}"#,
            r#"{"message":"unrecognized query parameter \"limt\""}"#,
        );

        assert!(matches!(unasserted, Judgement::StatusOnly(_)));
    }

    /// Measured against what was actually on port 3000 of the machine this was
    /// written on: an unrelated service, answering a 404 in an envelope close
    /// enough to this one's to be mistaken for it.
    #[test]
    fn refuses_a_base_url_that_is_serving_something_else() {
        assert!(answers_liveness(
            200,
            r#"{"status":"ok","uptime_seconds":4}"#
        ));
        assert!(answers_liveness(
            200,
            r#"{"status":"ok","uptimeSeconds":4}"#
        ));

        assert!(!answers_liveness(
            404,
            r#"{"message":"Route GET:/health/live not found","error":"Not Found","statusCode":404}"#
        ));
        assert!(!answers_liveness(200, r#"{"status":"degraded"}"#));
        assert!(!answers_liveness(200, "<html>a proxy page</html>"));
        assert!(!answers_liveness(503, r#"{"status":"ok"}"#));
    }

    /// The pages are compared over a real socket, and the walk ends where the
    /// cursor does rather than at a page count.
    #[tokio::test]
    async fn walks_a_listing_to_exhaustion_and_compares_every_page() {
        let oracle = Stub::start(
            (500, "unexpected"),
            &[
                (PAGE, 200, r#"{"items":[],"nextCursor":"p2"}"#),
                (
                    &format!("{PAGE}?cursor=p2"),
                    200,
                    r#"{"items":[],"nextCursor":null}"#,
                ),
            ],
        )
        .await;
        let candidate = Stub::start(
            (500, "unexpected"),
            &[
                (PAGE, 200, r#"{"items":[],"next_cursor":"p2"}"#),
                (
                    &format!("{PAGE}?cursor=p2"),
                    200,
                    r#"{"items":[],"next_cursor":null}"#,
                ),
            ],
        )
        .await;

        let report = walked(&oracle, &candidate).await;

        assert!(report.agreed(), "{}", report.text());
        assert!(
            report.text().contains("2 requests compared"),
            "{}",
            report.text()
        );
    }

    /// A cursor only one side would accept ends that walk, rather than pairing
    /// two different pages from there on.
    #[tokio::test]
    async fn stops_the_walk_where_the_two_cursors_diverge() {
        let oracle = Stub::start((200, r#"{"items":[],"nextCursor":"theirs"}"#), &[]).await;
        let candidate = Stub::start((200, r#"{"items":[],"next_cursor":"ours"}"#), &[]).await;

        let report = walked(&oracle, &candidate).await;
        let text = report.text();

        assert!(
            text.contains(r#"next cursor Some("theirs") against Some("ours")"#),
            "{text}"
        );
        assert!(text.contains("1 requests compared"), "{text}");
    }

    #[tokio::test]
    async fn a_field_that_differs_on_a_page_is_reported_with_its_path() {
        let oracle = Stub::start(
            (
                200,
                r#"{"items":[{"suppliedShares":"1"}],"nextCursor":null}"#,
            ),
            &[],
        )
        .await;
        let candidate = Stub::start(
            (
                200,
                r#"{"items":[{"supplied_shares":"2"}],"next_cursor":null}"#,
            ),
            &[],
        )
        .await;

        let report = walked(&oracle, &candidate).await;

        assert!(
            report
                .text()
                .contains(r#"items[0].supplied_shares: "1" against "2""#),
            "{}",
            report.text()
        );
    }

    /// Driving the whole corpus: every refusal and both probes, then every
    /// listing the one-wallet fold produces.
    #[tokio::test]
    async fn replays_the_whole_corpus_against_two_services_that_agree() {
        let refusal = r#"{"message":"limit must be between 1 and 200","error":"Bad Request","status_code":400}"#;
        let ready = r#"{"status":"ok","checks":[]}"#;
        let answers: &[(&str, u16, &str)] = &[("/health/ready", 200, ready)];

        let oracle = Stub::start((400, refusal), answers).await;
        let candidate = Stub::start((400, refusal), answers).await;

        let report = replay(&config(&oracle, &candidate), &fold()).await.unwrap();

        assert!(report.agreed(), "{}", report.text());
        assert!(
            report.text().contains("31 requests compared"),
            "{}",
            report.text()
        );
    }

    /// The guard that stops a run against whatever else is on that port from
    /// reading as a wall of findings.
    #[tokio::test]
    async fn refuses_to_replay_against_a_service_that_is_not_one_of_these() {
        let ours = Stub::start((200, "{}"), &[]).await;
        let stranger = Stub::start(
            (404, "{}"),
            &[(
                "/health/live",
                404,
                r#"{"message":"Route GET:/health/live not found"}"#,
            )],
        )
        .await;

        let refused = replay(&config(&ours, &stranger), &fold())
            .await
            .unwrap_err();

        assert!(
            matches!(
                refused,
                Error::NotThisService {
                    which: "candidate",
                    status: 404,
                    ..
                }
            ),
            "{refused}"
        );
    }

    #[tokio::test]
    async fn an_empty_fold_is_refused_rather_than_reported_as_agreement() {
        let service = Stub::start((200, "{}"), &[]).await;
        let empty = Fold {
            wallets: Vec::new(),
            latest: 0,
        };

        let refused = replay(&config(&service, &service), &empty)
            .await
            .unwrap_err();

        assert!(matches!(refused, Error::EmptyFold(1)), "{refused}");
    }

    /// The sharpest single check in the gate: the HMAC construction is shared,
    /// so the two services hand back the same bytes or one of them changed it.
    #[test]
    fn one_cursor_walks_both_services_or_the_walk_stops() {
        assert_eq!(
            shared_cursor(Some("abc".to_owned()), Some("abc".to_owned())),
            Ok(Some("abc".to_owned()))
        );
        assert_eq!(shared_cursor(None, None), Ok(None));
        assert_eq!(
            shared_cursor(Some("abc".to_owned()), Some("xyz".to_owned())),
            Err(r#"next cursor Some("abc") against Some("xyz")"#.to_owned())
        );
        assert_eq!(
            shared_cursor(Some("abc".to_owned()), None),
            Err(r#"next cursor Some("abc") against None"#.to_owned())
        );
    }

    #[test]
    fn reads_each_services_spelling_of_the_next_cursor() {
        assert_eq!(
            next_cursor(r#"{"nextCursor":"abc"}"#, "nextCursor").as_deref(),
            Some("abc")
        );
        assert_eq!(
            next_cursor(r#"{"next_cursor":"abc"}"#, "next_cursor").as_deref(),
            Some("abc")
        );
        assert_eq!(next_cursor(r#"{"next_cursor":null}"#, "next_cursor"), None);
        assert_eq!(next_cursor("not json", "next_cursor"), None);
    }
}
