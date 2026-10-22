use crate::error::Error;
use crate::pool::{Pool, connection};

/// The relations a caller named that this database does not have.
///
/// `to_regclass` resolves through `search_path` and answers `NULL` for a name
/// that is not there, where a cast would raise instead.
const MISSING: &str = "SELECT name FROM unnest($1::text[]) AS t(name) \
     WHERE to_regclass(t.name) IS NULL";

/// Reports Postgres reachable, and holding the relations a caller reads.
///
/// `SELECT 1` answered the first half alone, so a database that had never been
/// migrated reported itself ready and then served 500s.
///
/// Deliberately through the pool rather than around it: what a caller needs to
/// know is whether the next *query* can be served, and a pool with no connection
/// to give is as unserviceable as a server that is down.
///
/// # Errors
///
/// [`Error::ConnectFailed`] if no connection can be had, [`Error::PingFailed`]
/// if the server refuses the query, [`Error::Missing`] naming every relation
/// this database does not have.
pub async fn ping(pool: &Pool, relations: &[&str]) -> Result<(), Error> {
    let rows = connection(pool)
        .await?
        .query(MISSING, &[&relations])
        .await
        .map_err(|source| Error::PingFailed { source })?;

    if rows.is_empty() {
        return Ok(());
    }

    let missing: Vec<&str> = rows.iter().map(|row| row.get(0)).collect();

    Err(Error::Missing {
        missing: missing.join(", "),
    })
}

#[cfg(test)]
mod tests {
    //! Against a real server, and against its catalogue rather than this
    //! deployment's schema: `pg_class` is there in any database, so these prove
    //! the mechanism without needing the migrations to have run.

    use super::*;
    use crate::pool::{Limits, build_pool};

    fn pool(url: &str) -> Pool {
        build_pool(url, Limits::default()).expect("a parseable URL")
    }

    fn served() -> Pool {
        pool(
            &std::env::var("POSTGRES_URL")
                .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned()),
        )
    }

    #[tokio::test]
    async fn takes_a_database_holding_every_relation_it_was_asked_for() {
        ping(&served(), &["pg_class", "pg_namespace"])
            .await
            .expect("both are catalogue relations");
    }

    #[tokio::test]
    async fn names_the_relation_the_migrations_did_not_build() {
        let refused = ping(&served(), &["pg_class", "indexer_cursor_typo"])
            .await
            .expect_err("one of the two is not there");

        assert_eq!(
            refused.to_string(),
            "Postgres is missing required relations: indexer_cursor_typo"
        );
    }

    #[tokio::test]
    async fn asking_for_nothing_still_proves_the_server_answers() {
        ping(&served(), &[]).await.expect("reachable");

        let refused = ping(&pool("postgres://postgres@127.0.0.1:1/nothing"), &[])
            .await
            .expect_err("port 1 is reserved");

        assert_eq!(refused.to_string(), "could not connect to Postgres");
    }
}
