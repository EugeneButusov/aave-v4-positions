//! What a run against Postgres actually does.
//!
//! refinery drives Postgres natively, so none of our code sits in the path —
//! which is the reason to pin this rather than a reason not to. The behaviour is
//! what the deployment depends on, `packages/postgres` used to assert it from
//! the TypeScript side, and it was deleted along with that runner. Until this
//! file nothing in the workspace covered Postgres at all, while ClickHouse had
//! five cases.
//!
//! An integration test rather than a module beside the adapter, because there is
//! no adapter: everything here is public — `postgres::build_pool`, refinery, and
//! the driver a checked-out connection derefs to.
//!
//! `clippy.toml` sets `allow-unwrap-in-tests`, and it does not reach here: that
//! relaxation applies to `#[cfg(test)]` items, and an integration test is an
//! ordinary crate that happens to live under `tests/`. Allowed once, at the top,
//! rather than by threading `?` through setup code whose failure means the
//! server is not running.
#![allow(clippy::unwrap_used)]

use refinery_core::{Migration, Runner};

/// A schema per test, not per file. `cargo` runs these concurrently and they
/// assert on the exact set of tables that exists, so a shared one has them
/// creating and dropping each other's fixtures — which is precisely how the
/// first version of this file failed.
fn schema(test: &str) -> String {
    format!("rust_migrate_{test}")
}

/// Dropped and recreated, so each test starts from nothing. The ledger is what
/// would otherwise carry state between runs and make these assertions depend on
/// whatever the last one left behind.
/// The pool is returned alongside the connection so the caller keeps it alive
/// for the length of the test; a connection outliving its pool is a shape worth
/// not depending on.
async fn target(test: &str) -> (postgres::Pool, postgres::Connection, String) {
    let schema = schema(test);
    let base = std::env::var("POSTGRES_URL")
        .unwrap_or_else(|_| "postgres://postgres@localhost:5432/postgres".to_owned());

    let admin = postgres::build_pool(&base).unwrap();
    postgres::connection(&admin)
        .await
        .unwrap()
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
        ))
        .await
        .unwrap();

    // `options` is libpq's, and tokio-postgres parses it out of the URL — which
    // is what lets a schema be chosen without `crates/postgres` growing a
    // parameter only a test would ever pass.
    let separator = if base.contains('?') { '&' } else { '?' };
    let pool = postgres::build_pool(&format!(
        "{base}{separator}options=-c%20search_path%3D{schema}"
    ))
    .unwrap();
    let client = postgres::connection(&pool).await.unwrap();

    (pool, client, schema)
}

async fn tables(client: &postgres::Client, schema: &str) -> Vec<String> {
    let rows = client
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = $1 ORDER BY table_name",
            &[&schema],
        )
        .await
        .unwrap();
    rows.iter().map(|row| row.get(0)).collect()
}

async fn run(client: &mut postgres::Connection, set: &[Migration]) -> Result<Vec<String>, String> {
    Runner::new(set)
        // Three derefs: the reference, the pooled object, and the wrapper it
        // holds. refinery implements its traits for the driver's own client.
        .run_async(&mut ***client)
        .await
        .map(|report| {
            report
                .applied_migrations()
                .iter()
                .map(ToString::to_string)
                .collect()
        })
        .map_err(|error| error.to_string())
}

fn at(name: &str, sql: &str) -> Migration {
    Migration::unapplied(name, sql).unwrap()
}

const WIDGETS: &str = "CREATE TABLE widgets (id text PRIMARY KEY)";
const GADGETS: &str = "CREATE TABLE gadgets (id text PRIMARY KEY)";

#[tokio::test]
async fn creates_the_ledger_even_when_there_is_nothing_to_apply() {
    let (_pool, mut client, schema) = target("empty").await;

    assert_eq!(run(&mut client, &[]).await.unwrap(), Vec::<String>::new());

    // refinery's own DDL, unlike ClickHouse where the default is unusable and
    // `AsyncMigrate` has to supply ours.
    assert_eq!(tables(&client, &schema).await, ["refinery_schema_history"]);
}

#[tokio::test]
async fn applies_each_migration_in_order_and_records_it() {
    let (_pool, mut client, schema) = target("in_order").await;
    let set = [at("V1__widgets", WIDGETS), at("V2__gadgets", GADGETS)];

    assert_eq!(
        run(&mut client, &set).await.unwrap(),
        ["V1__widgets", "V2__gadgets"]
    );
    assert_eq!(
        tables(&client, &schema).await,
        ["gadgets", "refinery_schema_history", "widgets"]
    );
}

#[tokio::test]
async fn applies_nothing_on_a_second_run() {
    let (_pool, mut client, _schema) = target("second_run").await;
    let set = [at("V1__widgets", WIDGETS), at("V2__gadgets", GADGETS)];

    run(&mut client, &set).await.unwrap();

    assert_eq!(run(&mut client, &set).await.unwrap(), Vec::<String>::new());
}

/// The case `packages/postgres/src/migrate.spec.ts` covered, and the one whose
/// answer changed with the port.
///
/// The TypeScript runner wrapped an entire run in one transaction, so a set that
/// failed partway left nothing at all. refinery commits each migration with its
/// own ledger row, so what precedes the failure stays and is recorded — the
/// ClickHouse behaviour, now on both — and the retry resumes at the one that
/// failed rather than redoing the ones that worked.
#[tokio::test]
async fn resumes_at_the_migration_that_failed() {
    let (_pool, mut client, schema) = target("partial").await;
    let broken = at("V2__gadgets", "CREATE TABLE gadgets (id text PRIMARY KEY,");

    run(&mut client, &[at("V1__widgets", WIDGETS), broken])
        .await
        .unwrap_err();

    assert_eq!(
        tables(&client, &schema).await,
        ["refinery_schema_history", "widgets"]
    );
    assert_eq!(
        run(
            &mut client,
            &[at("V1__widgets", WIDGETS), at("V2__gadgets", GADGETS)]
        )
        .await
        .unwrap(),
        ["V2__gadgets"]
    );
}

/// The one thing Postgres does that ClickHouse cannot: a migration is atomic in
/// itself, so a file failing on its second statement leaves neither.
///
/// Every migration in this repository is a single statement, which is what makes
/// the difference stop mattering in practice — but it is what the design notes
/// claim separates the two databases, so it is asserted rather than assumed.
#[tokio::test]
async fn rolls_back_within_a_migration_that_fails_partway() {
    let (_pool, mut client, schema) = target("rollback").await;
    let half_broken = at(
        "V1__widgets",
        "CREATE TABLE widgets (id text PRIMARY KEY); CREATE TABLE gadgets (id text PRIMARY KEY,",
    );

    run(&mut client, &[half_broken]).await.unwrap_err();

    // `widgets` was created and rolled back. ClickHouse would have kept it —
    // that is `clickhouse::tests::resumes_at_the_migration_that_failed`.
    assert_eq!(tables(&client, &schema).await, ["refinery_schema_history"]);
}

/// The one piece of Postgres code that is ours, and the message an operator
/// actually reads. `main` prints the chain, so the cause underneath matters as
/// much as the head — it is the half that names the port.
///
/// **The two failure paths separated when the pool became the only way in**, and
/// that is an improvement rather than a regression: a URL that will not parse is
/// now caught before anything dials, so it says so instead of being reported as
/// a connection that could not be made.
#[test]
fn refuses_a_url_that_will_not_parse_before_it_dials() {
    let error = postgres::build_pool("not-a-url").unwrap_err();

    assert_eq!(error.to_string(), "could not parse the Postgres URL");
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn says_it_could_not_connect_when_the_server_will_not_have_us() {
    let pool = postgres::build_pool("postgres://postgres@127.0.0.1:1/nope").unwrap();

    let error = postgres::connection(&pool).await.unwrap_err();

    assert_eq!(error.to_string(), "could not connect to Postgres");
    assert!(
        std::error::Error::source(&error).is_some(),
        "the cause is what names the port"
    );
}
