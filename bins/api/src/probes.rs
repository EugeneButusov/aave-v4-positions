//! What a deployment asks of this process, rather than what a caller asks.
//!
//! **The dependencies are named here, in a list, and that is the whole
//! registry.** `ops` owns the report and the paths; this owns which databases
//! this process answers for.

use axum::Router;

use crate::app::AppState;

pub(crate) fn routes(state: AppState) -> Router {
    let (uptime, shutdown) = (state.uptime, state.shutdown.clone());

    ops::probe_router(uptime, shutdown, move || {
        let state = state.clone();
        async move {
            // Side by side: a probe that serialises them reports the sum of
            // two timeouts. `join!` keeps the order the wire contract fixes.
            let (clickhouse, postgres) = tokio::join!(
                ops::check("clickhouse", clickhouse_client::ping(&state.clickhouse)),
                ops::check("postgres", ::postgres::ping(&state.postgres)),
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

    use crate::test_support::{get, handler, postgres, unreachable_postgres};

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
