//! What a deployment asks of this process, rather than what a caller asks.
//!
//! **The dependencies are named here, and that is the whole registry.** `ops`
//! owns the report and the paths; migrations own schema integrity, while this
//! composition root asks whether the databases can serve a query now.

use axum::Router;
use clickhouse_client::clickhouse::Client;
use postgres::{Limits, Pool};

use crate::app::AppState;

/// What readiness connects with, which is never what requests connect with.
pub(crate) struct Dependencies {
    clickhouse: Client,
    postgres: Pool,
}

/// A pool of its own, so request traffic cannot starve the probe.
///
/// `max_size: 1`, not a share of the request pool and not
/// `Timeouts::wait_millis(0)` — deadpool turns a zero wait into a non-blocking
/// `try_acquire` and would report a busy pool as a down database. Two probes in
/// flight at once means the last ping did not return inside the orchestrator's
/// period, which is itself not-ready.
///
/// # Errors
///
/// Whatever [`postgres::build_pool`] refuses the URL for.
pub(crate) fn dependencies(
    clickhouse: Client,
    postgres_url: &str,
    limits: Limits,
) -> Result<Dependencies, postgres::Error> {
    Ok(from_pool(
        clickhouse,
        postgres::build_pool(
            postgres_url,
            Limits {
                max_size: 1,
                ..limits
            },
        )?,
    ))
}

/// Builds the capability checks over connections reserved for readiness.
///
/// Kept as a separate seam because the tests deliberately pass an exhausted
/// request pool to prove why production constructs a dedicated one.
pub(crate) fn from_pool(clickhouse: Client, postgres: Pool) -> Dependencies {
    Dependencies {
        clickhouse,
        postgres,
    }
}

pub(crate) fn routes(state: AppState) -> Router {
    let (uptime, shutdown) = (state.uptime, state.shutdown.clone());

    ops::probe_router(uptime, shutdown, move || {
        let state = state.clone();
        async move {
            // Side by side: a probe that serialises them reports the sum of
            // two timeouts. `join!` keeps the order the wire contract fixes.
            let (clickhouse, postgres) = tokio::join!(
                ops::check(
                    "clickhouse",
                    clickhouse_client::ping(&state.probes.clickhouse)
                ),
                ops::check("postgres", ::postgres::ping(&state.probes.postgres)),
            );
            vec![clickhouse, postgres]
        }
    })
}

#[cfg(test)]
mod tests {
    //! Against real servers, because the wiring is all that is left to get
    //! wrong: `ops` proves the report and the shutdown in isolation, and what
    //! these add is that the names, the order and the two `ping`s behind them
    //! reach the databases this process actually opens.

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use postgres::Limits;

    use super::{Dependencies, dependencies, from_pool};
    use crate::app::{self as app, App};
    use crate::test_support::app::{
        clickhouse, get, handler, postgres, postgres_url, state, unreachable_postgres,
    };

    fn ready() -> Request<Body> {
        Request::builder()
            .uri("/health/ready")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn reports_both_databases_up_against_the_real_servers() {
        let (status, body, _) = get(handler(postgres()), ready()).await;

        assert_eq!(status, StatusCode::OK);
        // Byte for byte, because the report is the contract.
        assert_eq!(
            body,
            r#"{"status":"ok","checks":[{"name":"clickhouse","status":"up"},{"name":"postgres","status":"up"}]}"#
        );
    }

    #[tokio::test]
    async fn readiness_answers_while_every_request_connection_is_held() {
        let limits = Limits::default();
        let requests = ::postgres::build_pool(&postgres_url(), limits).unwrap();

        // Held for the rest of the case, so the request pool has nothing left.
        let mut held = Vec::new();
        for _ in 0..limits.max_size {
            held.push(::postgres::connection(&requests).await.unwrap());
        }

        // Which is what the probe did before it had a pool of its own.
        let (status, body) = probe(from_pool(clickhouse(), requests.clone())).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            body.contains(r#"{"name":"postgres","status":"down","error":"could not connect"#),
            "{body}"
        );

        let (status, _) = probe(dependencies(clickhouse(), &postgres_url(), limits).unwrap()).await;
        assert_eq!(status, StatusCode::OK);
    }

    async fn probe(probes: Dependencies) -> (StatusCode, String) {
        let handler = app::handler(App {
            probes,
            ..state(postgres())
        });

        let (status, body, _) = get(handler, ready()).await;
        (status, body)
    }

    #[tokio::test]
    async fn names_postgres_when_only_postgres_is_unreachable() {
        // Which is the half a single aggregated boolean would lose, and the
        // reason the report carries names at all.
        let (status, body, _) = get(handler(unreachable_postgres()), ready()).await;

        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(body.starts_with(r#"{"status":"degraded","#), "{body}");
        assert!(
            body.contains(r#"{"name":"clickhouse","status":"up"}"#),
            "{body}"
        );
        assert!(
            body.contains(r#"{"name":"postgres","status":"down","error":"#),
            "{body}"
        );
    }
}
