//! What a probe answers, and nothing about how it is served.
//!
//! These types are the wire contract, not an internal shape: a deployment's
//! manifests and whatever watches them read these exact keys. They are matched
//! field for field against what the TypeScript service emits today, down to
//! `error` being **absent** rather than null on a check that passed.

use serde::Serialize;

/// The process is running. That is all liveness may claim.
///
/// One variant, because a wedged process fails to answer at all — a liveness
/// probe that could report anything else would be reporting on a dependency,
/// and a dependency outage should drain traffic rather than have kubelet
/// restart every replica.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Alive {
    Ok,
}

/// The body of `GET /health/live`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Liveness {
    pub status: Alive,
    pub uptime_seconds: u64,
}

/// Whether one dependency answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Up,
    Down,
}

/// One dependency's answer, named so a failing probe says which one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckResult {
    pub name: &'static str,
    pub status: CheckStatus,
    /// Why it failed, and **omitted entirely when it did not**.
    ///
    /// Not `null`: a passing check serialises to two keys, which is what the
    /// TypeScript emits and therefore what the differential compares against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Runs one dependency's check and names the answer.
///
/// **The whole error chain, not just its head**, for the reason `bins/migrate`
/// prints one: the top line says which stage failed and the cause underneath is
/// the one naming the socket or the refusal. A probe body is often the only
/// place an operator sees either.
pub async fn check<E>(name: &'static str, probe: impl Future<Output = Result<(), E>>) -> CheckResult
where
    E: std::error::Error,
{
    match probe.await {
        Ok(()) => CheckResult {
            name,
            status: CheckStatus::Up,
            error: None,
        },
        Err(error) => CheckResult {
            name,
            status: CheckStatus::Down,
            error: Some(chain(&error)),
        },
    }
}

/// The chain, skipping any link already spelled out by the one above it.
///
/// Two conventions live side by side in the ecosystem: `thiserror` messages
/// here name only their own stage and leave the cause to `source()`, while
/// `deadpool`'s interpolate theirs. Walking a mixed chain naively prints the
/// bottom half twice — measured against the real refusal, which came out as
/// "error connecting to server: error connecting to server: Connection
/// refused". A containment test reconciles both without either side having to
/// know about the other.
fn chain(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();

    while let Some(next) = cause {
        let text = next.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        cause = next.source();
    }

    message
}

/// Why readiness answered the way it did.
///
/// `shutting_down` outranks a failing dependency, because the two say different
/// things to whoever is reading: one is this pod being removed on purpose, the
/// other is something wrong. Both are served with a 503.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Ok,
    Degraded,
    ShuttingDown,
}

/// The body of `GET /health/ready`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub status: Readiness,
    /// In the order the checks were listed, so a reader comparing two responses
    /// is comparing rows rather than searching for them.
    pub checks: Vec<CheckResult>,
}

impl Report {
    /// **The checks still run while draining, and are still reported.**
    ///
    /// Only the top-line status is overridden. A pod that is going away is
    /// still worth asking about — the last readiness body before a rolling
    /// update finishes is sometimes the only evidence of what it saw.
    #[must_use]
    pub fn new(checks: Vec<CheckResult>, draining: bool) -> Self {
        let status = if draining {
            Readiness::ShuttingDown
        } else if checks.iter().all(|check| check.status == CheckStatus::Up) {
            Readiness::Ok
        } else {
            Readiness::Degraded
        };

        Self { status, checks }
    }
}

#[cfg(test)]
mod tests {
    //! The shapes, asserted against serialised bytes rather than the structs.
    //!
    //! Every literal below was measured against the running TypeScript service
    //! rather than read off its types — see the module doc on
    //! [`crate::probe`] for what was captured and how.

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("connection refused")]
    struct Refused;

    #[derive(Debug, thiserror::Error)]
    #[error("could not check out a connection")]
    struct Unavailable(#[source] Refused);

    fn json<T: Serialize>(value: &T) -> String {
        serde_json::to_string(value).unwrap()
    }

    fn up(name: &'static str) -> CheckResult {
        CheckResult {
            name,
            status: CheckStatus::Up,
            error: None,
        }
    }

    #[test]
    fn a_passing_check_has_two_keys_and_no_null_error() {
        assert_eq!(
            json(&up("clickhouse")),
            r#"{"name":"clickhouse","status":"up"}"#
        );
    }

    #[tokio::test]
    async fn names_the_dependency_and_the_reason_it_failed() {
        let result = check("clickhouse", async { Err::<(), _>(Refused) }).await;

        assert_eq!(
            json(&result),
            r#"{"name":"clickhouse","status":"down","error":"connection refused"}"#
        );
    }

    /// `deadpool`'s convention: the message already contains the source's.
    #[derive(Debug, thiserror::Error)]
    #[error("could not create an object: {0}")]
    struct Verbose(#[source] Refused);

    #[tokio::test]
    async fn says_a_cause_once_even_when_the_message_already_spells_it_out() {
        // Measured: walking this chain naively produced "error connecting to
        // server: error connecting to server: Connection refused", which reads
        // as two failures rather than one.
        let result = check("postgres", async { Err::<(), _>(Verbose(Refused)) }).await;

        assert_eq!(
            result.error.as_deref(),
            Some("could not create an object: connection refused")
        );
    }

    #[tokio::test]
    async fn carries_the_cause_as_well_as_the_head() {
        // A pool that cannot hand out a connection says so, but the reason it
        // cannot is one level down and is the half worth reading.
        let result = check("postgres", async { Err::<(), _>(Unavailable(Refused)) }).await;

        assert_eq!(
            result.error.as_deref(),
            Some("could not check out a connection: connection refused")
        );
    }

    #[test]
    fn is_ok_only_when_every_check_is() {
        let report = Report::new(vec![up("clickhouse"), up("postgres")], false);

        assert_eq!(
            json(&report),
            r#"{"status":"ok","checks":[{"name":"clickhouse","status":"up"},{"name":"postgres","status":"up"}]}"#
        );
    }

    #[test]
    fn is_degraded_when_one_is_down_and_says_which() {
        let down = CheckResult {
            name: "clickhouse",
            status: CheckStatus::Down,
            error: Some("connection refused".to_owned()),
        };

        let report = Report::new(vec![down, up("postgres")], false);

        assert_eq!(report.status, Readiness::Degraded);
        assert_eq!(report.checks[0].name, "clickhouse");
        assert_eq!(
            report.checks[0].error.as_deref(),
            Some("connection refused")
        );
    }

    #[test]
    fn is_shutting_down_even_with_every_dependency_up() {
        // The distinction the status exists to draw: nothing is wrong, this pod
        // is being taken out of rotation.
        let report = Report::new(vec![up("clickhouse"), up("postgres")], true);

        assert_eq!(report.status, Readiness::ShuttingDown);
    }

    #[test]
    fn reports_the_checks_in_the_order_they_were_listed() {
        let report = Report::new(vec![up("clickhouse"), up("postgres")], false);

        let names: Vec<_> = report.checks.iter().map(|check| check.name).collect();
        assert_eq!(names, ["clickhouse", "postgres"]);
    }
}
