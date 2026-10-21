//! A stand-in service, on a port the operating system picks.
//!
//! What it buys is the driving loop over a real socket: the client, the pairing
//! of two answers, and the cursor walk, none of which a hand-built `Answer`
//! reaches.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};

pub(crate) struct Stub(SocketAddr);

#[derive(Clone)]
struct Answers {
    default: (u16, String),
    by_target: Arc<HashMap<String, (u16, String)>>,

    /// When set, a versioned request waits here before it is answered. Two
    /// stubs sharing one barrier of two can only both answer if both requests
    /// are in flight at the same time.
    paired: Option<Arc<tokio::sync::Barrier>>,

    /// Milliseconds to hold a given target before answering it, so one walker
    /// can be made to finish after another by construction rather than by
    /// hoping about scheduling.
    slow: Arc<HashMap<String, u64>>,
}

impl Stub {
    /// One answer for anything not named, and an override per request target.
    /// Liveness is answered for free, because nothing is replayed until both
    /// services have passed it.
    pub(crate) async fn start(default: (u16, &str), by_target: &[(&str, u16, &str)]) -> Self {
        Self::listening(default, by_target, None, &[]).await
    }

    /// A stub that holds the named targets for that many milliseconds.
    pub(crate) async fn slow(default: (u16, &str), slow: &[(&str, u64)]) -> Self {
        Self::listening(default, &[], None, slow).await
    }

    /// A stub that will not answer a versioned request until the barrier it
    /// shares has as many waiters as it was built for.
    pub(crate) async fn paired(default: (u16, &str), barrier: &Arc<tokio::sync::Barrier>) -> Self {
        Self::listening(default, &[], Some(Arc::clone(barrier)), &[]).await
    }

    async fn listening(
        default: (u16, &str),
        by_target: &[(&str, u16, &str)],
        paired: Option<Arc<tokio::sync::Barrier>>,
        slow: &[(&str, u64)],
    ) -> Self {
        let mut answers: HashMap<String, (u16, String)> = by_target
            .iter()
            .map(|(target, status, body)| ((*target).to_owned(), (*status, (*body).to_owned())))
            .collect();

        answers
            .entry("/health/live".to_owned())
            .or_insert((200, r#"{"status":"ok","uptime_seconds":1}"#.to_owned()));

        let state = Answers {
            default: (default.0, default.1.to_owned()),
            by_target: Arc::new(answers),
            paired,
            slow: Arc::new(
                slow.iter()
                    .map(|(target, millis)| ((*target).to_owned(), *millis))
                    .collect(),
            ),
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = Router::new().fallback(answer).with_state(state);

        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        Self(address)
    }

    pub(crate) fn base(&self) -> String {
        format!("http://{}", self.0)
    }
}

async fn answer(State(answers): State<Answers>, uri: Uri) -> Response {
    let target = uri
        .path_and_query()
        .map_or_else(|| uri.path(), axum::http::uri::PathAndQuery::as_str);

    // Liveness is the precondition every run asks first, one service at a
    // time, so holding it at the barrier would deadlock before the pairing is
    // ever exercised.
    if let Some(barrier) = &answers.paired
        && !target.starts_with("/health")
    {
        barrier.wait().await;
    }

    if let Some(millis) = answers.slow.get(target) {
        tokio::time::sleep(std::time::Duration::from_millis(*millis)).await;
    }

    let (status, body) = answers
        .by_target
        .get(target)
        .unwrap_or(&answers.default)
        .clone();

    (StatusCode::from_u16(status).unwrap(), body).into_response()
}
