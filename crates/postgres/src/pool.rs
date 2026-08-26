use deadpool_postgres::{Manager, ManagerConfig, RecyclingMethod};

use crate::error::Error;

/// A pool of connections, shared by cloning.
///
/// An alias rather than a wrapper: `deadpool` is what every caller would
/// otherwise have to declare, and nothing of ours would sit between them and it.
pub type Pool = deadpool_postgres::Pool;

/// One connection, borrowed from a [`Pool`] and returned when it is dropped.
///
/// Derefs twice to [`Client`](crate::Client), which is what a caller that wants
/// the driver's own API reaches through — `&mut **connection` is what refinery
/// takes, for instance.
pub type Connection = deadpool_postgres::Object;

/// How many connections a process may hold open.
///
/// Ten, which is `postgres.js`'s default and therefore what this deployment's
/// connection budget is sized for today. A one-shot job that takes a single
/// connection is unaffected — the pool dials what is asked for and no more.
const MAX_CONNECTIONS: usize = 10;

/// Builds the pool every caller here connects through.
///
/// **Nothing connects here.** A pool dials on first checkout, so a service whose
/// database is briefly down still boots and reports itself not-ready — which is
/// what an orchestrator can act on. A process that refuses to start instead
/// turns a thirty-second outage into a crash loop, and `postgres.js` is lazy for
/// the same reason.
///
/// **And this is the only way in, including for a job that wants one connection
/// and then exits.** There was a second constructor handing out a bare
/// `tokio_postgres::Client`; taking one connection out of the pool does the same
/// job, and the bare one had a failure mode the pool does not: measured against a
/// server stopped and restarted underneath both, it answers "connection closed"
/// from the outage onward and never recovers, because the task driving its socket
/// has ended and nothing redials.
///
/// # Errors
///
/// [`Error::BadUrl`] if the URL will not parse, [`Error::PoolFailed`] if the pool
/// will not build. Neither can be an unreachable server, because neither step
/// talks to one — that is [`connection`]'s to report.
pub fn build_pool(url: &str) -> Result<Pool, Error> {
    let config = url
        .parse::<tokio_postgres::Config>()
        .map_err(|source| Error::BadUrl { source })?;

    // `Fast` skips a round trip on checkout. The manager still drops a connection
    // the driver has marked closed, so what this gives up is noticing a server
    // that died silently — which the readiness probe asks about anyway.
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
