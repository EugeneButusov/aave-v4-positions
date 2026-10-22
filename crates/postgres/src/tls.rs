use std::sync::{Arc, OnceLock};

use rustls::crypto::ring;
use rustls::{ClientConfig, RootCertStore};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::error::Error;

/// The connector every pool here dials through.
///
/// **The provider is named rather than resolved.** `ClientConfig::builder()`
/// asks rustls for a process default, which it derives from whichever of its
/// crypto features the whole dependency graph enables — so one future crate
/// pulling rustls with default features would add `aws-lc-rs` beside `ring` and
/// turn the first connection into a panic.
///
/// An empty trust store is not an error: it is what a container with no
/// `ca-certificates` has, and `sslmode=disable` and a `prefer` that falls back
/// both work without one. Every verification then fails, which is the answer.
///
/// # Errors
///
/// [`Error::NoTlsVersions`] if the provider supports none of the versions
/// rustls considers safe, which `ring` does.
pub(crate) fn connector() -> Result<MakeRustlsConnect, Error> {
    // Built once: reading the host's roots and compiling them into a verifier
    // measured 751 ms against 2 µs for a hundred cached calls, and this
    // workspace's suites build sixty-odd pools. Checked before rather than
    // through `get_or_init`, because the build can fail and `get_or_try_init`
    // is unstable — two threads racing here both get a working connector and
    // one of the two is dropped.
    static CONNECTOR: OnceLock<MakeRustlsConnect> = OnceLock::new();

    if let Some(ready) = CONNECTOR.get() {
        return Ok(ready.clone());
    }

    let mut store = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();

    if native.certs.is_empty() {
        tracing::warn!(
            errors = native.errors.len(),
            "no certificate roots on this host; every TLS handshake will be refused"
        );
    }
    store.add_parsable_certificates(native.certs);

    let config = ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|source| Error::NoTlsVersions { source })?
        .with_root_certificates(store)
        .with_no_client_auth();

    Ok(CONNECTOR
        .get_or_init(|| MakeRustlsConnect::new(config))
        .clone())
}

#[cfg(test)]
mod tests {
    //! Against a stub server rather than a real one, because what is in
    //! question is this side of the negotiation: with `NoTls` here the driver
    //! answered `can_connect` false and handed `sslmode=prefer` a raw stream
    //! without sending an `SSLRequest` at all.
    //!
    //! Certificate verification is rustls's and is not re-tested here. There is
    //! no configuration above that could switch it off.

    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    use crate::pool::{Limits, build_pool, connection};

    /// Int32 length 8, then the request code 80877103.
    const SSL_REQUEST: [u8; 8] = [0, 0, 0, 8, 4, 0xD2, 0x16, 0x2F];

    /// What the client sent first, and the byte after our `S`.
    async fn opening(sslmode: &str) -> ([u8; 8], Option<u8>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (reply, sent) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();

            let mut first = [0u8; 8];
            stream.read_exact(&mut first).await.unwrap();
            // `S` is "yes, TLS", which is the branch a raw stream never takes.
            stream.write_all(b"S").await.unwrap();

            let mut next = [0u8; 1];
            let next = stream.read_exact(&mut next).await.ok().map(|_| next[0]);

            let _ = reply.send((first, next));
        });

        let pool = build_pool(
            &format!("postgres://postgres@{address}/nothing?sslmode={sslmode}"),
            Limits::default(),
        )
        .unwrap();

        // Refused either way: nothing above speaks the rest of the protocol.
        let _ = connection(&pool).await;

        sent.await.unwrap()
    }

    #[tokio::test]
    async fn asks_for_tls_and_then_opens_a_handshake() {
        let (first, next) = opening("require").await;

        assert_eq!(first, SSL_REQUEST);
        // 0x16 is the TLS record type for a handshake, so the client answered
        // our `S` with a ClientHello rather than giving up or going plaintext.
        assert_eq!(next, Some(0x16), "a ClientHello");
    }

    #[tokio::test]
    async fn sends_no_ssl_request_when_the_url_asks_for_none() {
        let (first, _) = opening("disable").await;

        assert_ne!(first, SSL_REQUEST);
        // A StartupMessage: Int32 length, then protocol version 3.0.
        assert_eq!(first[4..8], [0, 3, 0, 0], "{first:?}");
    }
}
