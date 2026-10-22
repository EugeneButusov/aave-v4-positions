/// What a readiness check against ClickHouse can fail at.
///
/// Its own type rather than the driver's, because these messages reach an
/// unauthenticated body and `clickhouse::error::Error::BadResponse` is the
/// server's response quoted verbatim.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not reach ClickHouse")]
    Unreachable {
        #[source]
        source: clickhouse::error::Error,
    },
}
