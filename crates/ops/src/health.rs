//! What a probe answers, and nothing about how it is served.
//!
//! These types are the wire contract, not an internal shape: a deployment's
//! manifests and whatever watches them read these exact keys. They are matched
//! field for field against what the TypeScript service emits today, down to
//! `error` being **absent** rather than null on a check that passed.
//!
//! **One deliberate deviation: `uptime_seconds`, where the TypeScript says
//! `uptimeSeconds`.** It is the only multi-word key on this whole surface, so
//! honouring it meant a `rename_all` that governed exactly one field and left
//! the probes the one camel-cased thing in the crate. Nothing reads it — all
//! three compose healthchecks hit `/health/ready` and look only at the status
//! code, and a Kubernetes probe does the same — so the key has no consumer to
//! break. The differential gate is told about this one rather than left to
//! report it; see `docs/rust-migration.md`.

use serde::Serialize;

use crate::cause::error_message_with_causes;

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
///
/// No `rename_all`: the field below is snake_case on the wire, which is the
/// deviation the module header explains.
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
            error: Some(error_message_with_causes(&error)),
        },
    }
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
    //! The shapes, asserted against serialised bytes rather than the structs.
    //!
    //! Every literal below was measured against the running TypeScript service
    //! rather than read off its types — see the module doc on
    //! [`crate::probe`] for what was captured and how.

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
