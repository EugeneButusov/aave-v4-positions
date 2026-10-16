//! What a probe answers, and nothing about how it is served.
//!
//! These types are the wire contract: a deployment's manifests read these exact
//! keys, matched field for field against what the TypeScript emits, down to
//! `error` being absent rather than null on a check that passed.
//!
//! One deliberate deviation, `uptime_seconds` against the TypeScript's
//! `uptimeSeconds` — recorded for the differential gate in
//! `docs/rust-migration.md`.

use serde::Serialize;

use crate::cause::error_message_with_causes;

/// The process is running. That is all liveness may claim.
///
/// One variant: a wedged process fails to answer at all, and reporting on a
/// dependency here would have kubelet restart every replica over one outage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Alive {
    Ok,
}

/// The body of `GET /health/live`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
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
    /// Why it failed, and omitted entirely when it did not: a passing check
    /// serialises to two keys, which is what the differential compares against.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Runs one dependency's check and names the answer.
///
/// Carries the whole error chain, because a probe body is often the only place
/// an operator sees the cause.
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
            error: Some(error_message_with_causes(&error)),
        },
    }
}

/// Why readiness answered the way it did.
///
/// `shutting_down` outranks a failing dependency: one is this pod being removed
/// on purpose, the other is something wrong. Both are served with a 503.
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
    /// Only the top-line status is overridden while draining — the checks still
    /// run and are still reported.
    #[must_use]
    pub fn new(checks: Vec<CheckResult>, shutting_down: bool) -> Self {
        let status = if shutting_down {
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
    //! Asserted against serialised bytes rather than the structs. Every literal
    //! was measured against the running TypeScript service — see the module doc
    //! on [`crate::probe`].

    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("connection refused")]
    struct Refused;

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
