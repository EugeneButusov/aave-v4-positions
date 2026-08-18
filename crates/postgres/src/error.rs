/// What connecting can fail at.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not connect to Postgres")]
    Connect {
        #[source]
        source: tokio_postgres::Error,
    },
}
