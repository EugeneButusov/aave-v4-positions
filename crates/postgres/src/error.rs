/// What connecting can fail at.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not connect to Postgres")]
    ConnectFailed {
        #[source]
        source: tokio_postgres::Error,
    },

    #[error("could not parse the Postgres URL")]
    BadUrl {
        #[source]
        source: tokio_postgres::Error,
    },

    #[error("could not build the connection pool")]
    PoolFailed {
        #[source]
        source: deadpool_postgres::BuildError,
    },

    #[error("no connection available from the pool")]
    Unavailable {
        #[source]
        source: deadpool_postgres::PoolError,
    },

    #[error("the health query failed")]
    PingFailed {
        #[source]
        source: tokio_postgres::Error,
    },
}
