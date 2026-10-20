//! Two GETs against two base URLs.

use http_body_util::{BodyExt, Empty};
use hyper::Request;
use hyper::body::Bytes;
use hyper::http::uri::InvalidUri;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;

type Legacy = hyper_util::client::legacy::Client<HttpConnector, Empty<Bytes>>;

pub(crate) struct Client(Legacy);

pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("{0:?} is not a URL")]
    Target(String, #[source] InvalidUri),

    #[error("{0} did not answer")]
    Unanswered(String, #[source] hyper_util::client::legacy::Error),

    #[error("{0} answered a body that would not read")]
    Unreadable(String, #[source] hyper::Error),

    #[error("the request to {0} would not build")]
    Unbuildable(String, #[source] hyper::http::Error),
}

impl Client {
    pub(crate) fn new() -> Self {
        Self(hyper_util::client::legacy::Client::builder(TokioExecutor::new()).build_http())
    }

    /// # Errors
    ///
    /// [`Error`] naming the whole URL, because a run points at two services
    /// and a bare message does not say which one went quiet.
    pub(crate) async fn get(
        &self,
        base: &str,
        method: &str,
        target: &str,
    ) -> Result<Answer, Error> {
        let url = format!("{}{target}", base.trim_end_matches('/'));

        let uri = url
            .parse::<hyper::Uri>()
            .map_err(|invalid| Error::Target(url.clone(), invalid))?;

        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Empty::new())
            .map_err(|broken| Error::Unbuildable(url.clone(), broken))?;

        let response = self
            .0
            .request(request)
            .await
            .map_err(|silent| Error::Unanswered(url.clone(), silent))?;

        let status = response.status().as_u16();
        let body = response
            .into_body()
            .collect()
            .await
            .map_err(|torn| Error::Unreadable(url, torn))?
            .to_bytes();

        Ok(Answer {
            status,
            body: String::from_utf8_lossy(&body).into_owned(),
        })
    }
}
