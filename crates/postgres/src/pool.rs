use deadpool_postgres::{Manager, ManagerConfig, RecyclingMethod};

use crate::error::Error;

/// A pool of connections, shared by cloning.
///
/// An alias for the same reason [`crate::Client`] is one: `deadpool` is what
/// every caller would otherwise have to declare, and nothing of ours would sit
/// between them and it.
pub type Pool = deadpool_postgres::Pool;

/// How many connections a process may hold open.
///
/// Ten, which is `postgres.js`'s default and therefore what this deployment's
/// connection budget is sized for today. Changing it is a capacity decision
/// about the server, not a detail of the port.
const MAX_CONNECTIONS: usize = 10;

/// Builds the pool a long-lived process serves from.
///
/// **Nothing connects here**, and that is the difference from
/// [`crate::build_client`] rather than an oversight. A pool dials on first use,
/// so a service whose database is briefly down still boots and reports itself
/// not-ready — which is what an orchestrator can act on. A process that refuses
/// to start instead turns a thirty-second outage into a crash loop, and
/// `postgres.js` is lazy for the same reason.
///
/// [`crate::build_client`] stays for `bins/migrate`, which hands refinery one
/// raw connection and runs to completion.
///
/// # Errors
///
/// [`Error::BadUrl`] if the URL will not parse, [`Error::PoolFailed`] if the
/// pool will not build. Neither can be an unreachable server, because neither
/// step talks to one.
pub fn build_pool(url: &str) -> Result<Pool, Error> {
    let config = url
        .parse::<tokio_postgres::Config>()
        .map_err(|source| Error::BadUrl { source })?;

    // `Fast` skips a round trip on checkout. The manager still drops a
    // connection the driver has marked closed, so what this gives up is
    // noticing a server that died silently — which the readiness probe asks
    // about every ten seconds anyway.
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
