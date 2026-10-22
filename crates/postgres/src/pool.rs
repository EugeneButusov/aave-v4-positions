use std::time::Duration;

use deadpool_postgres::{Manager, ManagerConfig, RecyclingMethod, Runtime, Timeouts};

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
/// All four have to be said, because deadpool's own default is `None` at three
/// of them and `None` there means no timer at all — `pool.get()` parks for as
/// long as the pool is busy, and a dial against a black-holed host waits out
/// the OS. A pool with no deadline turns a slow database into a dead process
/// instead of a failed request.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_size: usize,

    /// Waiting for a connection another request holds.
    pub wait: Duration,

    /// Waiting for one to be dialled, which is where an unreachable host lands.
    pub create: Duration,
    pub recycle: Duration,

    /// `statement_timeout`, server-side, applied to every connection the pool
    /// dials.
    pub statement: Deadline,
}

/// How long a statement may run before the server cancels it.
///
/// An enum rather than an `Option<Duration>`, because `None` reads as "no
/// opinion" and would mean "no limit" — so the dangerous value is the one a
/// caller reaches for when they have not thought about it. Here the unbounded
/// case has to be named, and names itself in review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deadline {
    After(Duration),

    /// For a caller whose statements legitimately run as long as they run: a
    /// migration, where a deadline buys a half-applied schema rather than a
    /// bounded one. Not reachable from configuration, because this is a
    /// decision about what the statements are, not about a deployment.
    Unbounded,
}

impl Default for Limits {
    /// Sized for a request path, where every statement is a keyed read.
    ///
    /// `max_size` is the deployment's own figure. The number here used to be
    /// 10, described as `postgres.js`'s default — which it is, and which this
    /// deployment does not use.
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
/// [`Error::BadUrl`] if the URL will not parse, [`Error::PoolFailed`] if the
/// pool will not build. Neither can be an unreachable server.
pub fn build_pool(url: &str, limits: Limits) -> Result<Pool, Error> {
    let mut config = url
        .parse::<tokio_postgres::Config>()
        .map_err(|source| Error::BadUrl { source })?;

    if let Some(options) = options(config.get_options(), limits.statement) {
        config.options(options);
    }

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
        .max_size(limits.max_size)
        .timeouts(Timeouts {
            wait: Some(limits.wait),
            create: Some(limits.create),
            recycle: Some(limits.recycle),
        })
        // Required once any timeout is set, because a timeout needs a timer and
        // deadpool will not pick one for you. Omitting it is `NoRuntimeSpecified`
        // from `build`, which is the loud version of a pool with no deadlines.
        .runtime(Runtime::Tokio1)
        .build()
        .map_err(|source| Error::PoolFailed { source })
}

/// The `options` string a connection should carry, or `None` to leave whatever
/// the URL said alone.
///
/// Appends rather than assigns: a URL may carry `options` of its own, and
/// replacing them would drop a setting the deployment asked for.
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
/// [`Error::ConnectFailed`], whose source is the driver's account of why —
/// including [`Limits::wait`] elapsing with every connection still held.
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

    /// The URL's own `options` survive. Assigning instead of appending would
    /// drop whatever the deployment put there.
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

    /// A migration asks for none, because a deadline there buys a half-applied
    /// schema rather than a bounded one.
    /// The default is bounded, so a caller that does not think about it gets
    /// a deadline rather than none. That is the whole reason this is an enum.
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

    /// The figure that matters most, asserted rather than left to the doc: a
    /// positions request takes three checkouts, so at 10 the pool outran the
    /// budget it claimed to be sized for.
    #[test]
    fn is_sized_for_the_deployment_and_not_for_the_driver_it_came_from() {
        assert_eq!(Limits::default().max_size, 4);
    }
}
