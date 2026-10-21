//! A migrated schema per case, so cases can run at once.
//!
//! Here rather than beside either harness: both of this crate's Postgres ports
//! want one, and a second copy of it would be the fourth in the workspace.

use alloy_primitives::B256;
use postgres::{Pool, build_pool, connection};
use refinery_core::{Migration, Runner};

/// `CLAUDE.md`'s local invocation sets this; CI sets nothing and relies on the
/// fallback, so a suite that only passes locally is reading something CI will
/// not give it.
fn url() -> String {
    std::env::var("POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned())
}

pub(crate) async fn scratch(prefix: &str, case: &str) -> Pool {
    let base = url();
    let schema = format!("{prefix}_{case}");

    let admin = build_pool(&base).unwrap();
    connection(&admin)
        .await
        .unwrap()
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
        ))
        .await
        .unwrap();

    // `search_path` is a startup parameter, which is why the schema is created
    // over a second pool rather than this one.
    let pool = build_pool(&scoped(&base, &schema)).unwrap();

    let set: Vec<Migration> = migrations::POSTGRES
        .iter()
        .flat_map(|source| source.files)
        .map(|embedded| Migration::unapplied(embedded.label, embedded.sql).unwrap())
        .collect();

    let mut client = connection(&pool).await.unwrap();
    Runner::new(&set).run_async(&mut **client).await.unwrap();
    drop(client);

    pool
}

/// The spelling both tables' `CHECK (… ~ '^0x[0-9a-f]{64}$')` demand.
pub(crate) fn lower(hash: B256) -> String {
    format!("{hash:#x}")
}

/// `base` with the schema appended as a startup option.
///
/// Its own function because the separator has two answers and a deployment URL
/// carrying `?sslmode=require` is the one that takes the second: appending `?`
/// to a URL that already has one builds a parameter nobody set and a search
/// path that is never applied.
fn scoped(base: &str, schema: &str) -> String {
    let separator = if base.contains('?') { '&' } else { '?' };

    format!("{base}{separator}options=-c%20search_path%3D{schema}")
}

#[cfg(test)]
mod tests {
    use super::scoped;

    #[test]
    fn appends_the_schema_with_whichever_separator_the_url_wants() {
        assert_eq!(
            scoped("postgres://host/db", "s"),
            "postgres://host/db?options=-c%20search_path%3Ds"
        );
        assert_eq!(
            scoped("postgres://host/db?sslmode=require", "s"),
            "postgres://host/db?sslmode=require&options=-c%20search_path%3Ds"
        );
    }
}
