use std::time::Duration;

use deadpool_postgres::{Manager, ManagerConfig, RecyclingMethod, Runtime, Timeouts};
use tokio_postgres::config::SslMode;

use crate::error::Error;

/// A pool of connections, shared by cloning.
pub type Pool = deadpool_postgres::Pool;

/// One connection, borrowed from a [`Pool`] and returned when it is dropped.
///
/// Derefs twice to [`Client`](crate::Client), which is what refinery takes as
/// `&mut **connection`.
pub type Connection = deadpool_postgres::Object;

/// What a caller may be made to wait for, at each of the four places it can.
///
/// All four are named because deadpool defaults three of them to `None`, which
/// is no timer rather than a default one.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_size: usize,
    pub wait: Duration,
    pub create: Duration,
    pub recycle: Duration,
    pub statement: Deadline,
}

/// `statement_timeout`, server-side.
///
/// Not an `Option<Duration>`: `None` reads as "no opinion" and would mean "no
/// limit", putting the dangerous value where a caller reaches by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deadline {
    After(Duration),
    Unbounded,
}

impl Default for Limits {
    /// A request path. `max_size` is this deployment's figure, not the
    /// driver's — the 10 here before was `postgres.js`'s and went unused.
    fn default() -> Self {
        Self {
            max_size: 4,
            wait: Duration::from_millis(250),
            create: Duration::from_secs(2),
            recycle: Duration::from_secs(1),
            statement: Deadline::After(Duration::from_secs(5)),
        }
    }
}

/// Builds the pool every caller here connects through, and the only way in.
///
/// Nothing connects: a pool dials on first checkout, so a service whose
/// database is briefly down still boots and reports itself not-ready rather
/// than turning a short outage into a crash loop.
///
/// # Errors
///
/// [`Error::BadUrl`] if the URL will not parse — including an `sslmode` this
/// driver does not spell and any `sslrootcert`, both of which it reads as
/// unknown — and [`Error::PoolFailed`] if the pool will not build. Neither can
/// be an unreachable server.
pub fn build_pool(url: &str, limits: Limits) -> Result<Pool, Error> {
    let mut config = url
        .parse::<tokio_postgres::Config>()
        .map_err(|source| Error::BadUrl { source })?;

    // `tokio-postgres` defaults to `prefer`, which silently falls back to a
    // plaintext session when the server refuses TLS. This service instead
    // requires TLS unless the URL explicitly names `sslmode=disable`.
    if config.get_ssl_mode() == SslMode::Prefer {
        config.ssl_mode(SslMode::Require);
    }

    if let Some(options) = options(config.get_options(), limits.statement) {
        config.options(options);
    }

    // `Fast` skips a round trip on checkout. The manager still drops a
    // connection the driver has marked closed, so what this gives up is
    // noticing a server that died silently — which readiness asks about anyway.
    let manager_config = ManagerConfig {
        recycling_method: RecyclingMethod::Fast,
    };
    let manager = if config.get_ssl_mode() == SslMode::Disable {
        Manager::from_config(config, tokio_postgres::NoTls, manager_config)
    } else {
        Manager::from_config(config, crate::tls::connector()?, manager_config)
    };

    Pool::builder(manager)
        .max_size(limits.max_size)
        .timeouts(Timeouts {
            wait: Some(limits.wait),
            create: Some(limits.create),
            recycle: Some(limits.recycle),
        })
        // Required once any timeout is set; without it `build` refuses.
        .runtime(Runtime::Tokio1)
        .build()
        .map_err(|source| Error::PoolFailed { source })
}

/// Appended, not assigned: the schema-per-case harnesses pass their
/// `search_path` here, and assigning would send every one of them to `public`.
fn options(carried: Option<&str>, statement: Deadline) -> Option<String> {
    let Deadline::After(statement) = statement else {
        return None;
    };

    let setting = format!("-c statement_timeout={}", statement.as_millis());

    Some(match carried {
        Some(carried) => format!("{carried} {setting}"),
        None => setting,
    })
}

/// Takes one connection, dialling if the pool has none to hand.
///
/// # Errors
///
/// [`Error::ConnectFailed`], including [`Limits::wait`] elapsing.
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
        let pool =
            build_pool("postgres://postgres@127.0.0.1:1/nothing", Limits::default()).unwrap();

        pool.clone().close();

        assert!(pool.is_closed(), "closing the clone closed this one");
    }

    #[test]
    fn carries_the_statement_deadline_as_a_connection_option() {
        let setting = options(None, Deadline::After(Duration::from_millis(1_500)));

        assert_eq!(setting.as_deref(), Some("-c statement_timeout=1500"));
    }

    #[test]
    fn keeps_the_options_the_url_already_carried() {
        let setting = options(
            Some("-c search_path=alt"),
            Deadline::After(Duration::from_secs(5)),
        );

        assert_eq!(
            setting.as_deref(),
            Some("-c search_path=alt -c statement_timeout=5000")
        );
    }

    #[test]
    fn is_bounded_unless_a_caller_says_otherwise() {
        assert_eq!(
            Limits::default().statement,
            Deadline::After(Duration::from_secs(5))
        );
        assert_ne!(Limits::default().statement, Deadline::Unbounded);
    }

    #[test]
    fn asks_for_no_deadline_only_when_a_caller_names_it() {
        assert_eq!(options(None, Deadline::Unbounded), None);
        assert_eq!(
            options(Some("-c search_path=alt"), Deadline::Unbounded),
            None
        );
    }

    #[test]
    fn hands_the_pool_a_deadline_at_every_place_it_can_wait() {
        let limits = Limits::default();
        let pool = build_pool("postgres://postgres@127.0.0.1:1/nothing", limits).unwrap();

        assert_eq!(pool.status().max_size, limits.max_size);
        assert_eq!(pool.timeouts().wait, Some(limits.wait));
        assert_eq!(pool.timeouts().create, Some(limits.create));
        assert_eq!(pool.timeouts().recycle, Some(limits.recycle));
    }

    /// Three checkouts per request, so 10 outran the budget it claimed.
    #[test]
    fn is_sized_for_the_deployment_and_not_for_the_driver_it_came_from() {
        assert_eq!(Limits::default().max_size, 4);
    }
}
