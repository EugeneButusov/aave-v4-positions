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
}

impl Stub {
    /// One answer for anything not named, and an override per request target.
    /// Liveness is answered for free, because nothing is replayed until both
    /// services have passed it.
    pub(crate) async fn start(default: (u16, &str), by_target: &[(&str, u16, &str)]) -> Self {
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

    let (status, body) = answers
        .by_target
        .get(target)
        .unwrap_or(&answers.default)
        .clone();

    (StatusCode::from_u16(status).unwrap(), body).into_response()
}
