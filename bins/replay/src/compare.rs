//! What the two services must agree about, and the little they need not.
//!
//! Values and key order are compared exactly. Three classes are not, and each
//! is named rather than discovered: an instant is compared as an instant
//! because the two services spell one differently, a clock-derived number is
//! compared within a tolerance because two requests cannot be issued at the
//! same moment, and `uptime_seconds` is each process's own.

use std::collections::{BTreeSet, HashMap};
use std::fmt;

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::json::Json;

/// One place two answers differ.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Difference {
    path: String,
    detail: String,
}

impl fmt::Display for Difference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(formatter, "the body: {}", self.detail)
        } else {
            write!(formatter, "{}: {}", self.path, self.detail)
        }
    }
}

/// What one path is allowed to differ by.
enum Rule {
    /// A number of seconds, or an instant, measured at two different moments.
    Within(u64),

    /// Comparable by type and nothing else.
    Kind,
}

/// The exceptions, by path. A rule is a whole path rather than a field name,
/// so one written for `sync.age_seconds` does not forgive a field of that name
/// somewhere else.
pub(crate) struct Rules(HashMap<&'static str, Rule>);

impl Rules {
    /// No exceptions: every value and every key position compared as it
    /// arrived. What a refusal and a page with `as_of` set are held to.
    pub(crate) fn exact() -> Self {
        Self(HashMap::new())
    }

    /// A page carries three clocks. `sync` is the indexer's progress and moves
    /// whatever the request asked for; the other two are pinned by `as_of`,
    /// which also nulls `pricing` on both sides.
    pub(crate) fn page(skew: u64, pinned: bool) -> Self {
        let mut rules = HashMap::from([("sync.age_seconds", Rule::Within(skew))]);

        if !pinned {
            rules.insert("valued_at", Rule::Within(skew));
            rules.insert("pricing.age_seconds", Rule::Within(skew));
        }

        Self(rules)
    }

    /// These two processes were started minutes apart, so the one number
    /// liveness carries is not a quantity they share.
    pub(crate) fn liveness() -> Self {
        Self(HashMap::from([("uptime_seconds", Rule::Kind)]))
    }

    fn get(&self, path: &str) -> Option<&Rule> {
        self.0.get(path)
    }
}

/// Every difference, not the first: a report naming one field per run takes as
/// many runs as there are faults.
pub(crate) fn compare(oracle: &Json, candidate: &Json, rules: &Rules) -> Vec<Difference> {
    let mut found = Vec::new();
    walk("", oracle, candidate, rules, &mut found);
    found
}

/// Which parameters a refusal names, whichever service wrote the text.
///
/// The two messages differ by design, so a 400 is compared on its status and
/// on this set. Matched as substrings, so a message that quotes a bad *value*
/// containing a parameter's name would be read as naming it.
pub(crate) fn parameters_named(message: &str) -> BTreeSet<&'static str> {
    const PARAMETERS: [(&str, &str); 8] = [
        ("chainId", "chain_id"),
        ("chain_id", "chain_id"),
        ("user", "user"),
        ("spoke", "spoke"),
        ("limit", "limit"),
        ("cursor", "cursor"),
        ("asOf", "as_of"),
        ("as_of", "as_of"),
    ];

    PARAMETERS
        .iter()
        .filter(|(spelling, _)| message.contains(spelling))
        .map(|(_, canonical)| *canonical)
        .collect()
}

fn walk(path: &str, oracle: &Json, candidate: &Json, rules: &Rules, found: &mut Vec<Difference>) {
    if let Some(rule) = rules.get(path) {
        if let Some(detail) = rule.check(oracle, candidate) {
            found.push(Difference {
                path: path.to_owned(),
                detail,
            });
        }

        return;
    }

    match (oracle, candidate) {
        (Json::Object(left), Json::Object(right)) => object(path, left, right, rules, found),
        (Json::Array(left), Json::Array(right)) => array(path, left, right, rules, found),
        (Json::String(left), Json::String(right)) => text(path, left, right, found),
        _ if oracle == candidate => {}
        _ => found.push(Difference {
            path: path.to_owned(),
            detail: format!("{oracle} against {candidate}"),
        }),
    }
}

/// The key *sequence*, not the key set: two files here call field order part of
/// the contract.
fn object(
    path: &str,
    left: &[(String, Json)],
    right: &[(String, Json)],
    rules: &Rules,
    found: &mut Vec<Difference>,
) {
    let ours: Vec<&str> = left.iter().map(|(key, _)| key.as_str()).collect();
    let theirs: Vec<&str> = right.iter().map(|(key, _)| key.as_str()).collect();

    if ours != theirs {
        found.push(Difference {
            path: path.to_owned(),
            detail: format!("keys {ours:?} against {theirs:?}"),
        });
    }

    for (key, value) in left {
        if let Some((_, other)) = right.iter().find(|(name, _)| name == key) {
            walk(&field(path, key), value, other, rules, found);
        }
    }
}

fn array(path: &str, left: &[Json], right: &[Json], rules: &Rules, found: &mut Vec<Difference>) {
    if left.len() != right.len() {
        found.push(Difference {
            path: path.to_owned(),
            detail: format!("{} items against {}", left.len(), right.len()),
        });
    }

    for (at, (ours, theirs)) in left.iter().zip(right).enumerate() {
        walk(&element(path, at), ours, theirs, rules, found);
    }
}

/// Two instants are compared as instants: one service writes three fractional
/// digits always, the other writes the digits it has.
fn text(path: &str, left: &str, right: &str, found: &mut Vec<Difference>) {
    let detail = match (instant(left), instant(right)) {
        (Some(ours), Some(theirs)) if ours == theirs => return,
        (Some(ours), Some(theirs)) => format!("{ours} against {theirs}"),
        _ if left == right => return,
        _ => format!("{left:?} against {right:?}"),
    };

    found.push(Difference {
        path: path.to_owned(),
        detail,
    });
}

impl Rule {
    fn check(&self, oracle: &Json, candidate: &Json) -> Option<String> {
        match *self {
            Self::Within(skew) => within(skew, oracle, candidate),
            Self::Kind => (oracle.kind() != candidate.kind())
                .then(|| format!("{oracle} against {candidate}, which are not the same kind")),
        }
    }
}

fn within(skew: u64, oracle: &Json, candidate: &Json) -> Option<String> {
    match (oracle, candidate) {
        (Json::Null, Json::Null) => None,
        (Json::Number(left), Json::Number(right)) => match (left.as_i64(), right.as_i64()) {
            (Some(left), Some(right)) => {
                let apart = (i128::from(left) - i128::from(right)).abs();

                (apart > i128::from(skew))
                    .then(|| format!("{left} against {right}, {apart}s apart"))
            }
            _ => Some(format!("{oracle} against {candidate}, and not both counts")),
        },
        (Json::String(left), Json::String(right)) => match (instant(left), instant(right)) {
            (Some(ours), Some(theirs)) => {
                let apart = (ours - theirs).whole_seconds().abs();

                (apart > i64::try_from(skew).unwrap_or(i64::MAX))
                    .then(|| format!("{ours} against {theirs}, {apart}s apart"))
            }
            _ => Some(format!("{left:?} against {right:?}, and not both instants")),
        },
        _ => Some(format!("{oracle} against {candidate}")),
    }
}

fn instant(text: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(text, &Rfc3339).ok()
}

fn field(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn element(path: &str, at: usize) -> String {
    format!("{path}[{at}]")
}

#[cfg(test)]
mod tests {
    use crate::json::Json;

    use super::{Difference, Rules, compare, parameters_named};

    fn json(text: &str) -> Json {
        Json::parse(text).unwrap()
    }

    fn differences(oracle: &str, candidate: &str, rules: &Rules) -> Vec<String> {
        compare(&json(oracle), &json(candidate), rules)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    fn details(found: &[Difference]) -> Vec<String> {
        found.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn two_identical_bodies_differ_nowhere() {
        let body = r#"{"user":"0xab","items":[{"limit":1}]}"#;

        assert!(differences(body, body, &Rules::exact()).is_empty());
    }

    #[test]
    fn names_the_path_of_a_value_that_differs() {
        assert_eq!(
            differences(
                r#"{"items":[{"price_usd":"1.0"},{"price_usd":"2.0"}]}"#,
                r#"{"items":[{"price_usd":"1.0"},{"price_usd":"2.5"}]}"#,
                &Rules::exact(),
            ),
            [r#"items[1].price_usd: "2.0" against "2.5""#]
        );
    }

    /// Key *order*, which a set comparison would pass. The error envelope's
    /// order is contractual.
    #[test]
    fn the_same_keys_in_another_order_differ() {
        assert_eq!(
            differences(
                r#"{"message":"no","error":"Bad Request"}"#,
                r#"{"error":"Bad Request","message":"no"}"#,
                &Rules::exact(),
            ),
            [r#"the body: keys ["message", "error"] against ["error", "message"]"#]
        );
    }

    #[test]
    fn a_key_only_one_side_serves_differs() {
        assert_eq!(
            differences(r#"{"a":1,"b":2}"#, r#"{"a":1}"#, &Rules::exact()),
            [r#"the body: keys ["a", "b"] against ["a"]"#]
        );
        assert_eq!(
            differences(r#"{"a":1}"#, r#"{"a":1,"b":2}"#, &Rules::exact()),
            [r#"the body: keys ["a"] against ["a", "b"]"#]
        );
    }

    #[test]
    fn a_page_of_another_length_differs() {
        assert_eq!(
            differences(
                r#"{"items":[1,2,3]}"#,
                r#"{"items":[1,2]}"#,
                &Rules::exact()
            ),
            ["items: 3 items against 2"]
        );
    }

    /// Reported together: one fault per run is one run per fault.
    #[test]
    fn reports_every_difference_rather_than_the_first() {
        assert_eq!(
            differences(
                r#"{"a":1,"b":2,"c":3}"#,
                r#"{"a":9,"b":2,"c":8}"#,
                &Rules::exact()
            ),
            ["a: 1 against 9", "c: 3 against 8"]
        );
    }

    /// The whole reason instants are not compared as text: three fixed
    /// fractional digits against the digits there are.
    #[test]
    fn one_instant_in_two_spellings_is_one_instant() {
        let found = differences(
            r#"{"valued_at":"2026-07-25T17:20:00.000Z"}"#,
            r#"{"valued_at":"2026-07-25T17:20:00Z"}"#,
            &Rules::exact(),
        );

        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn two_instants_a_second_apart_differ() {
        assert_eq!(
            differences(
                r#"{"valued_at":"2026-07-25T17:20:00.000Z"}"#,
                r#"{"valued_at":"2026-07-25T17:20:01Z"}"#,
                &Rules::exact(),
            )
            .len(),
            1
        );
    }

    #[test]
    fn the_indexer_clock_is_forgiven_up_to_the_skew_and_no_further() {
        let oracle = r#"{"sync":{"age_seconds":100}}"#;

        assert!(
            differences(
                oracle,
                r#"{"sync":{"age_seconds":103}}"#,
                &Rules::page(3, false)
            )
            .is_empty()
        );
        assert_eq!(
            differences(
                oracle,
                r#"{"sync":{"age_seconds":104}}"#,
                &Rules::page(3, false)
            ),
            ["sync.age_seconds: 100 against 104, 4s apart"]
        );
    }

    /// With `as_of` set the page's two other clocks are the caller's number,
    /// so they go back to being compared exactly.
    #[test]
    fn a_pinned_page_holds_its_valuation_clock_exactly() {
        let oracle = r#"{"valued_at":"2026-07-25T17:20:00.000Z"}"#;
        let candidate = r#"{"valued_at":"2026-07-25T17:20:02Z"}"#;

        assert!(differences(oracle, candidate, &Rules::page(5, false)).is_empty());
        assert_eq!(
            differences(oracle, candidate, &Rules::page(5, true)).len(),
            1
        );
    }

    #[test]
    fn liveness_compares_its_uptime_by_kind_only() {
        let oracle = r#"{"status":"ok","uptime_seconds":9}"#;

        assert!(
            differences(
                oracle,
                r#"{"status":"ok","uptime_seconds":40000}"#,
                &Rules::liveness()
            )
            .is_empty()
        );
        assert_eq!(
            differences(
                oracle,
                r#"{"status":"ok","uptime_seconds":"40000"}"#,
                &Rules::liveness()
            ),
            [r#"uptime_seconds: 9 against "40000", which are not the same kind"#]
        );
    }

    /// An exception is a path, so the same field name elsewhere in the payload
    /// is still compared exactly.
    #[test]
    fn an_exception_does_not_reach_a_field_of_that_name_elsewhere() {
        assert_eq!(
            differences(
                r#"{"items":[{"sync":{"age_seconds":1}}]}"#,
                r#"{"items":[{"sync":{"age_seconds":2}}]}"#,
                &Rules::page(60, false),
            ),
            ["items[0].sync.age_seconds: 1 against 2"]
        );
    }

    /// The path of a difference at the root, which has no field to name.
    #[test]
    fn a_body_that_is_the_wrong_shape_outright_is_named_as_the_body() {
        let found = compare(&json("[]"), &json("{}"), &Rules::exact());

        assert_eq!(details(&found), ["the body: [] against {}"]);
    }

    #[test]
    fn reads_the_parameters_out_of_either_services_wording() {
        assert_eq!(
            parameters_named("\u{2716} Invalid input\n  \u{2192} at user"),
            ["user"].into_iter().collect()
        );
        assert_eq!(
            parameters_named(r#"user must be a 20-byte hex address, got "0xzz""#),
            ["user"].into_iter().collect()
        );
        assert_eq!(
            parameters_named("asOf must be between 1774277159 and 4102444800"),
            ["as_of"].into_iter().collect()
        );
        assert_eq!(
            parameters_named(
                r#"limit must be between 1 and 200, got "0"; unrecognized query parameter "spoke""#
            ),
            ["limit", "spoke"].into_iter().collect()
        );
        assert!(parameters_named("Internal Server Error").is_empty());
    }
}
