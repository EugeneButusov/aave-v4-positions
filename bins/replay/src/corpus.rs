//! What gets replayed: every wallet in the fold, and the requests that are
//! meant to be refused.

use crate::compare::Rules;
use crate::fold::Fold;

/// The ceiling the route publishes. A page asked for at exactly this is the
/// one that proves the bound is inclusive.
const MAX_LIMIT: u32 = 200;

/// 2026-03-23T14:45:59Z — the Main Spoke's first log and the floor `as_of`
/// takes. A page pinned here is one where nothing has happened yet.
const GENESIS: u32 = 1_774_277_159;

/// 2100-01-01, the ceiling `as_of` takes.
const MAX_AS_OF: u64 = 4_102_444_800;

/// The sizes worth asking for beside the default one. 1 pages everything, 7 is
/// an odd size no boundary falls on by luck, and `MAX_LIMIT` is the published
/// ceiling.
const LIMITS: [u32; 3] = [1, 7, MAX_LIMIT];

/// Which service a target is spelled for. The only difference is `asOf`
/// against `as_of`.
#[derive(Clone, Copy)]
pub(crate) enum Dialect {
    Oracle,
    Candidate,
}

/// How a body is compared once the statuses have been.
pub(crate) enum Body {
    /// Every value and every key position, under the one key transform.
    Json(Rules),

    /// A refusal's wording is each service's own; which parameters it names is
    /// not. The oracle reports the first fault and the candidate reports all
    /// of them, so the oracle's set has to be contained rather than equal.
    Parameters,
}

/// One request, in both spellings.
pub(crate) struct Case {
    pub(crate) label: String,
    pub(crate) method: &'static str,
    pub(crate) oracle: String,
    pub(crate) candidate: String,
    pub(crate) body: Body,
}

/// One listing, walked page by page at run time because each page's cursor
/// comes out of the one before it.
pub(crate) struct Listing {
    chain_id: u32,
    user: String,
    spoke: Option<String>,
    limit: Option<u32>,
    as_of: Option<u32>,
}

impl Listing {
    pub(crate) fn new(
        chain_id: u32,
        user: &str,
        spoke: Option<&str>,
        limit: Option<u32>,
        as_of: Option<u32>,
    ) -> Self {
        Self {
            chain_id,
            user: user.to_owned(),
            spoke: spoke.map(ToOwned::to_owned),
            limit,
            as_of,
        }
    }

    pub(crate) fn target(&self, prefix: &str, dialect: Dialect, cursor: Option<&str>) -> String {
        let mut query = String::new();

        if let Some(spoke) = &self.spoke {
            append(&mut query, "spoke", spoke);
        }
        if let Some(limit) = self.limit {
            append(&mut query, "limit", &limit.to_string());
        }
        if let Some(as_of) = self.as_of {
            append(&mut query, as_of_key(dialect), &as_of.to_string());
        }
        if let Some(cursor) = cursor {
            append(&mut query, "cursor", cursor);
        }

        format!("{}{query}", self.path(prefix))
    }

    /// With `as_of` set, the page's valuation clock is the caller's number and
    /// its price clock is null, so both go back to being compared exactly.
    pub(crate) fn rules(&self, skew: u64) -> Rules {
        Rules::page(skew, self.as_of.is_some())
    }

    pub(crate) fn label(&self) -> String {
        let spoke = self.spoke.as_deref().map_or_else(
            || "every spoke".to_owned(),
            |spoke| format!("spoke {spoke}"),
        );
        let limit = self
            .limit
            .map_or_else(|| "default".to_owned(), |limit| limit.to_string());
        let as_of = match self.as_of {
            None => "now".to_owned(),
            Some(GENESIS) => "genesis".to_owned(),
            Some(instant) => instant.to_string(),
        };

        format!("{} {spoke} limit={limit} as_of={as_of}", self.user)
    }

    fn path(&self, prefix: &str) -> String {
        format!(
            "{}/chains/{}/users/{}/positions",
            mount(prefix),
            self.chain_id,
            self.user
        )
    }
}

/// Every wallet the fold has, crossed with every page size and three valuation
/// instants, plus each wallet's own Spokes.
pub(crate) fn listings(fold: &Fold, chain_id: u32, wallets: usize) -> Vec<Listing> {
    let take = if wallets == 0 {
        fold.wallets.len()
    } else {
        wallets
    };
    let instants = [None, Some(GENESIS), Some(fold.latest)];
    let mut listings = Vec::new();

    for wallet in fold.wallets.iter().take(take) {
        for limit in page_sizes(wallet.positions) {
            for as_of in instants {
                listings.push(Listing::new(chain_id, &wallet.user, None, limit, as_of));
            }
        }

        for spoke in &wallet.spokes {
            for as_of in [None, Some(fold.latest)] {
                listings.push(Listing::new(
                    chain_id,
                    &wallet.user,
                    Some(spoke),
                    None,
                    as_of,
                ));
            }
        }
    }

    listings
}

/// Which page sizes can tell the two services apart for a wallet this size.
///
/// A size at or above the wallet's position count returns the one full page, so
/// every such size is the same request under another name — a wallet with two
/// positions is walked at 1 and at the default, where the other two sizes would
/// be that same single page twice more. The default is always asked for,
/// because the limit a caller does not name is the one most of them use.
fn page_sizes(positions: u64) -> Vec<Option<u32>> {
    let mut sizes = vec![None];

    sizes.extend(
        LIMITS
            .into_iter()
            .filter(|limit| u64::from(*limit) < positions)
            .map(Some),
    );

    sizes
}

/// Where the versioned routes hang, given what a deployment wrote in
/// `API_GLOBAL_PREFIX`. The same arithmetic both services do, because a corpus
/// pointed at the wrong mount compares two 404s and calls it agreement.
pub(crate) fn mount(prefix: &str) -> String {
    match prefix.trim_matches('/') {
        "" => "/v1".to_owned(),
        prefix => format!("/{prefix}/v1"),
    }
}

/// The probes, the refusals, and the two shapes of unmatched request.
///
/// `user` is a wallet the fold has, so a case that is meant to fail on one
/// parameter is not also failing on the address.
pub(crate) fn cases(prefix: &str, chain_id: u32, user: &str, fold: &Fold) -> Vec<Case> {
    let mount = mount(prefix);
    let positions = format!("{mount}/chains/{chain_id}/users/{user}/positions");
    let milliseconds = u64::from(fold.latest) * 1_000;

    let mut cases = vec![
        probe("liveness", "/health/live", Rules::liveness()),
        probe("readiness", "/health/ready", Rules::exact()),
        exact(
            "a path neither service serves",
            "GET",
            format!("{mount}/nothing/here"),
        ),
        exact(
            "a method this path does not serve",
            "POST",
            positions.clone(),
        ),
        exact(
            "a chain nothing has indexed",
            "GET",
            format!("{mount}/chains/987654/users/{user}/positions"),
        ),
    ];

    let refusals = [
        (
            "an address that is not one",
            format!("{mount}/chains/{chain_id}/users/0xnot-an-address/positions"),
        ),
        (
            "a chain id that is not a number",
            format!("{mount}/chains/mainnet/users/{user}/positions"),
        ),
        ("a limit below the floor", format!("{positions}?limit=0")),
        (
            "a limit above the ceiling",
            format!("{positions}?limit={}", MAX_LIMIT + 1),
        ),
        (
            "a limit that is not a number",
            format!("{positions}?limit=lots"),
        ),
        ("a parameter neither serves", format!("{positions}?limt=5")),
        (
            "the same parameter twice",
            format!("{positions}?limit=1&limit=2"),
        ),
        (
            "a cursor neither issued",
            format!("{positions}?cursor=not-a-cursor"),
        ),
        (
            "two faults in one request",
            format!("{mount}/chains/{chain_id}/users/0xzz/positions?limit=0"),
        ),
    ];

    cases.extend(refusals.into_iter().map(|(label, target)| Case {
        label: label.to_owned(),
        method: "GET",
        oracle: target.clone(),
        candidate: target,
        body: Body::Parameters,
    }));

    // The three that differ by the one query key, so each is spelled twice.
    let instants = [
        ("an as_of below genesis", "1".to_owned()),
        ("an as_of above the ceiling", (MAX_AS_OF + 1).to_string()),
        ("an as_of in milliseconds", milliseconds.to_string()),
    ];

    cases.extend(instants.into_iter().map(|(label, value)| Case {
        label: label.to_owned(),
        method: "GET",
        oracle: format!("{positions}?asOf={value}"),
        candidate: format!("{positions}?as_of={value}"),
        body: Body::Parameters,
    }));

    cases
}

fn probe(label: &str, path: &str, rules: Rules) -> Case {
    Case {
        label: label.to_owned(),
        method: "GET",
        oracle: path.to_owned(),
        candidate: path.to_owned(),
        body: Body::Json(rules),
    }
}

/// A case whose body is held to the whole transform. The strictest rule, used
/// wherever neither service's wording has been declared its own.
fn exact(label: &str, method: &'static str, target: String) -> Case {
    Case {
        label: label.to_owned(),
        method,
        oracle: target.clone(),
        candidate: target,
        body: Body::Json(Rules::exact()),
    }
}

fn as_of_key(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Oracle => "asOf",
        Dialect::Candidate => "as_of",
    }
}

fn append(query: &mut String, key: &str, value: &str) {
    query.push(if query.is_empty() { '?' } else { '&' });
    query.push_str(key);
    query.push('=');
    query.push_str(value);
}

#[cfg(test)]
mod tests {
    use crate::fold::{Fold, Wallet};

    use super::{Dialect, GENESIS, Listing, cases, listings};

    fn fold() -> Fold {
        Fold {
            wallets: vec![
                Wallet {
                    user: "0xaa".to_owned(),
                    spokes: vec!["0x01".to_owned(), "0x02".to_owned()],
                    positions: 9,
                },
                Wallet {
                    user: "0xbb".to_owned(),
                    spokes: vec!["0x01".to_owned()],
                    positions: 1,
                },
            ],
            latest: 1_780_000_000,
        }
    }

    fn listing(spoke: Option<&str>, limit: Option<u32>, as_of: Option<u32>) -> Listing {
        Listing::new(1, "0xaa", spoke, limit, as_of)
    }

    /// The one query key that differs, and the one the gate exists to record.
    #[test]
    fn the_two_dialects_differ_in_the_one_key() {
        let listing = listing(None, Some(50), Some(1_780_000_000));

        assert_eq!(
            listing.target("api", Dialect::Oracle, None),
            "/api/v1/chains/1/users/0xaa/positions?limit=50&asOf=1780000000"
        );
        assert_eq!(
            listing.target("api", Dialect::Candidate, None),
            "/api/v1/chains/1/users/0xaa/positions?limit=50&as_of=1780000000"
        );
    }

    #[test]
    fn a_listing_with_nothing_asked_for_carries_no_query() {
        assert_eq!(
            listing(None, None, None).target("api", Dialect::Candidate, None),
            "/api/v1/chains/1/users/0xaa/positions"
        );
    }

    /// The cursor is appended rather than substituted, so a page two request
    /// still carries the filters that scoped page one — which is what the
    /// signature over the scope is checked against.
    #[test]
    fn a_resumed_page_keeps_the_filters_that_scoped_it() {
        assert_eq!(
            listing(Some("0x01"), Some(7), None).target("api", Dialect::Candidate, Some("abc")),
            "/api/v1/chains/1/users/0xaa/positions?spoke=0x01&limit=7&cursor=abc"
        );
    }

    #[test]
    fn a_pinned_listing_is_compared_without_the_clock_exceptions() {
        use crate::compare::compare;
        use crate::json::Json;

        let oracle = Json::parse(r#"{"valued_at":"2026-07-25T17:20:00.000Z"}"#).unwrap();
        let candidate = Json::parse(r#"{"valued_at":"2026-07-25T17:20:03Z"}"#).unwrap();

        assert!(compare(&oracle, &candidate, &listing(None, None, None).rules(5)).is_empty());
        assert!(
            !compare(
                &oracle,
                &candidate,
                &listing(None, None, Some(1_780_000_000)).rules(5)
            )
            .is_empty()
        );
    }

    /// Nine positions is paged at the default, at 1 and at 7 — not at 200,
    /// which returns the same single page as the default. One position is
    /// paged at the default alone.
    #[test]
    fn crosses_the_useful_page_sizes_with_every_instant() {
        let listings = listings(&fold(), 1, 0);

        assert_eq!(listings.len(), (3 * 3 + 2 * 2) + (3 + 2));
    }

    /// A size at or above the wallet's position count is the same request as
    /// the default one, and this fold's wallets mostly hold one or two.
    #[test]
    fn asks_only_for_page_sizes_that_can_page() {
        assert_eq!(super::page_sizes(1), [None]);
        assert_eq!(super::page_sizes(2), [None, Some(1)]);
        assert_eq!(super::page_sizes(8), [None, Some(1), Some(7)]);
        assert_eq!(
            super::page_sizes(201),
            [None, Some(1), Some(7), Some(super::MAX_LIMIT)]
        );
    }

    #[test]
    fn replays_every_wallet_unless_told_how_many() {
        assert_eq!(listings(&fold(), 1, 1).len(), 3 * 3 + 2 * 2);
        assert_eq!(
            listings(&fold(), 1, 99).len(),
            listings(&fold(), 1, 0).len()
        );
    }

    #[test]
    fn names_the_instant_a_listing_is_pinned_at() {
        assert!(listing(None, None, None).label().ends_with("as_of=now"));
        assert!(
            listing(None, None, Some(GENESIS))
                .label()
                .ends_with("as_of=genesis")
        );
    }

    /// `asOf` is the one case whose two spellings are both in the corpus; the
    /// rest are one target used twice.
    #[test]
    fn the_refusals_that_name_an_instant_are_spelled_for_each_service() {
        let cases = cases("api", 1, "0xaa", &fold());
        let instants: Vec<&super::Case> = cases
            .iter()
            .filter(|case| case.label.contains("as_of"))
            .collect();

        assert_eq!(instants.len(), 3);
        for case in instants {
            assert!(case.oracle.contains("asOf="), "{}", case.oracle);
            assert!(case.candidate.contains("as_of="), "{}", case.candidate);
        }
    }

    /// The arithmetic both services do, mirrored here. An empty prefix asks
    /// for none, which is `/v1` and not `//v1`.
    #[test]
    fn hangs_the_routes_where_the_deployment_mounts_them() {
        assert_eq!(super::mount("api"), "/api/v1");
        assert_eq!(super::mount("/gateway/"), "/gateway/v1");
        assert_eq!(super::mount(""), "/v1");
        assert_eq!(super::mount("/"), "/v1");
    }

    #[test]
    fn every_versioned_target_carries_the_prefix_and_no_probe_does() {
        let cases = cases("gateway", 1, "0xaa", &fold());

        for case in &cases {
            if case.oracle.starts_with("/health") {
                continue;
            }

            assert!(case.oracle.starts_with("/gateway/v1/"), "{}", case.oracle);
        }
    }

    #[test]
    fn asks_both_probes_at_the_paths_neither_versions() {
        let cases = cases("api", 1, "0xaa", &fold());
        let probes: Vec<&str> = cases
            .iter()
            .map(|case| case.oracle.as_str())
            .filter(|target| target.starts_with("/health"))
            .collect();

        assert_eq!(probes, ["/health/live", "/health/ready"]);
    }
}
