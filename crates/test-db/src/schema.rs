//! The schema a case owns, and the pool whose connections open inside it.

use postgres::{Limits, Pool, build_pool, connection};

use crate::corpus;

/// `SHOW max_identifier_length`. Two names that truncate equal are one schema,
/// and the cases sharing it interfere.
const MAX_IDENTIFIER: usize = 63;

/// Dropped and recreated on construction, so a case starts from nothing
/// however the last run ended.
pub struct Scratch {
    pool: Pool,
    schema: String,
}

impl Scratch {
    /// With the Postgres corpus applied.
    pub async fn migrated(prefix: &str, case: &str) -> Self {
        let scratch = Self::empty(prefix, case).await;

        connection(&scratch.pool)
            .await
            .unwrap()
            .batch_execute(&corpus::sql())
            .await
            .unwrap();

        scratch
    }

    /// Empty, for the migrator's own tests, where applying the corpus is the
    /// thing under test.
    pub async fn empty(prefix: &str, case: &str) -> Self {
        let base = url();
        let schema = format!("{prefix}_{case}");

        assert!(
            schema.len() <= MAX_IDENTIFIER,
            "schema name is {} bytes: {schema}",
            schema.len()
        );

        let admin = build_pool(&base, Limits::default()).unwrap();
        connection(&admin)
            .await
            .unwrap()
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
            ))
            .await
            .unwrap();

        Self {
            // `search_path` arrives as a startup parameter, which is why the
            // schema is created over the pool above and used through this one.
            pool: build_pool(&scoped(&base, &schema), Limits::default()).unwrap(),
            schema,
        }
    }

    #[must_use]
    pub fn pool(&self) -> &Pool {
        &self.pool
    }

    #[must_use]
    pub fn schema(&self) -> &str {
        &self.schema
    }
}

/// `CLAUDE.md`'s local invocation sets this; CI sets nothing and relies on the
/// fallback, so a suite that only passes locally is reading something CI will
/// not give it. `sslmode=disable` because [`build_pool`] promotes `prefer` to
/// `require` and neither server offers TLS.
fn url() -> String {
    std::env::var("POSTGRES_URL").unwrap_or_else(|_| {
        "postgres://postgres@localhost:5432/postgres?sslmode=disable".to_owned()
    })
}

/// `base` with the schema appended as a startup option. Appending `?` to a URL
/// already carrying `sslmode` would build a parameter nobody set, and a search
/// path never applied.
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
