use deadpool_postgres::{Manager, ManagerConfig, RecyclingMethod};

use crate::error::Error;

/// A pool of connections, shared by cloning.
pub type Pool = deadpool_postgres::Pool;

/// One connection, borrowed from a [`Pool`] and returned when it is dropped.
///
/// Derefs twice to [`Client`](crate::Client), which is what refinery takes as
/// `&mut **connection`.
pub type Connection = deadpool_postgres::Object;

/// `postgres.js`'s default, which this deployment's budget is sized for.
const MAX_CONNECTIONS: usize = 10;

/// Builds the pool every caller here connects through, and the only way in.
///
/// Nothing connects: a pool dials on first checkout, so a service whose
/// database is briefly down still boots and reports itself not-ready rather
/// than turning a short outage into a crash loop.
///
/// # Errors
///
/// [`Error::BadUrl`] if the URL will not parse, [`Error::PoolFailed`] if the
/// pool will not build. Neither can be an unreachable server.
pub fn build_pool(url: &str) -> Result<Pool, Error> {
    let config = url
        .parse::<tokio_postgres::Config>()
        .map_err(|source| Error::BadUrl { source })?;

    // `Fast` skips a round trip on checkout. The manager still drops a
    // connection the driver has marked closed, so what this gives up is
    // noticing a server that died silently — which readiness asks about anyway.
    let manager = Manager::from_config(
        config,
        tokio_postgres::NoTls,
        ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        },
    );

    Pool::builder(manager)
        .max_size(MAX_CONNECTIONS)
        .build()
        .map_err(|source| Error::PoolFailed { source })
}

/// Takes one connection, dialling if the pool has none to hand.
///
/// # Errors
///
/// [`Error::ConnectFailed`], whose source is the driver's account of why.
pub async fn connection(pool: &Pool) -> Result<Connection, Error> {
    pool.get()
        .await
        .map_err(|source| Error::ConnectFailed { source })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// That cloning a [`Pool`] shares one rather than making a second.
    ///
    /// The readiness handler clones its pool on every call, and `Clone` says
    /// nothing about which of the two it is. Closing rather than counting, so
    /// this needs no server.
    #[tokio::test]
    async fn a_cloned_pool_is_the_same_pool() {
        let pool = build_pool("postgres://postgres@127.0.0.1:1/nothing").unwrap();
        let clone = pool.clone();

        clone.close();

        assert!(pool.is_closed(), "the clone closed a pool of its own");
        assert!(
            matches!(connection(&pool).await, Err(Error::ConnectFailed { .. })),
            "the original still handed out a connection"
        );
    }
}
