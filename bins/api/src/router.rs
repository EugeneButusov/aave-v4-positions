//! Every path this service answers on.
//!
//! Probes and the two fallbacks, for now. The versioned routes and the prefix
//! they sit under arrive with the positions endpoint; a prefix with nothing
//! beneath it is configuration that cannot be observably wrong.
//!
//! **The dependencies are named here, in a list, and that is the whole
//! registry.** `ops` owns the report and the paths; this owns which databases
//! this process answers for.

use axum::Router;

use crate::app::AppState;

pub(crate) fn build(state: AppState) -> Router {
    let (uptime, drain) = (state.uptime, state.drain.clone());

    ops::probe_router(uptime, drain, move || {
        let state = state.clone();
        async move {
            // Side by side rather than one after the other, as the TypeScript's
            // `Promise.all` does: neither answer depends on the other, and a
            // probe that serialises them reports the sum of two timeouts.
            // `join!` keeps the order of the results, which the wire contract
            // fixes.
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
    //! Against real servers, because the wiring is the only thing left to get
    //! wrong: `ops` already proves the report and the drain in isolation, and
    //! what these add is that the names, the order and the two `ping`s behind
    //! them are hooked up to the databases this process actually opens.

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
        // Byte for byte what the TypeScript service answered when both were
        // reachable, measured rather than transcribed from its DTOs.
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
