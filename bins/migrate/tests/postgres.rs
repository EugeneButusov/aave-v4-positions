//! What a run against Postgres actually does.
//!
//! refinery drives Postgres natively, so none of our code sits in the path —
//! which is the reason to pin this rather than a reason not to. The behaviour
//! is what the deployment depends on, and `packages/postgres` asserted it until
//! that runner was deleted.
//!
//! An integration test rather than a module beside the adapter, because there
//! is no adapter: everything here is public.
//!
//! `allow-unwrap-in-tests` does not reach an integration test, which is an
//! ordinary crate under `tests/` — hence the allow at the top.
#![allow(clippy::unwrap_used)]

use refinery_core::{Migration, Runner};

/// A schema per test, not per file: `cargo` runs these concurrently and they
/// assert on the exact set of tables, which is how the first version failed.
fn schema(test: &str) -> String {
    format!("rust_migrate_{test}")
}

/// Dropped and recreated, so each test starts from nothing — the ledger would
/// otherwise carry state between runs. The pool comes back with the connection
/// so the caller keeps it alive for the length of the test.
async fn target(test: &str) -> (postgres::Pool, postgres::Connection, String) {
    let schema = schema(test);
    let base = std::env::var("POSTGRES_URL").unwrap_or_else(|_| {
        "postgres://postgres@localhost:5432/postgres?sslmode=disable".to_owned()
    });

    let admin = postgres::build_pool(&base, postgres::Limits::default()).unwrap();
    postgres::connection(&admin)
        .await
        .unwrap()
        .batch_execute(&format!(
            "DROP SCHEMA IF EXISTS {schema} CASCADE; CREATE SCHEMA {schema}"
        ))
        .await
        .unwrap();

    // `options` is libpq's and tokio-postgres parses it out of the URL, so no
    // parameter only a test would pass has to reach `crates/postgres`.
    let separator = if base.contains('?') { '&' } else { '?' };
    let pool = postgres::build_pool(
        &format!("{base}{separator}options=-c%20search_path%3D{schema}"),
        postgres::Limits::default(),
    )
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
        // Three derefs, because refinery implements its traits for the
        // driver's own client.
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

    // refinery's own DDL, unlike ClickHouse where ours has to be supplied.
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

/// The one case whose answer changed with the port: the TypeScript wrapped an
/// entire run in one transaction, where refinery commits each migration with
/// its own ledger row — so a retry resumes at the one that failed.
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

/// The one thing Postgres does that ClickHouse cannot: a migration is atomic
/// in itself, so a file failing on its second statement leaves neither. Every
/// migration here is a single statement, so this is asserted, not relied on.
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
/// reads. The two failure paths separated when the pool became the only way
/// in: a URL that will not parse is caught before anything dials.
#[test]
fn refuses_a_url_that_will_not_parse_before_it_dials() {
    let error = postgres::build_pool("not-a-url", postgres::Limits::default()).unwrap_err();

    assert_eq!(error.to_string(), "could not parse the Postgres URL");
    assert!(std::error::Error::source(&error).is_some());
}

#[tokio::test]
async fn says_it_could_not_connect_when_the_server_will_not_have_us() {
    let pool = postgres::build_pool(
        "postgres://postgres@127.0.0.1:1/nope",
        postgres::Limits::default(),
    )
    .unwrap();

    let error = postgres::connection(&pool).await.unwrap_err();

    assert_eq!(error.to_string(), "could not connect to Postgres");
    assert!(
        std::error::Error::source(&error).is_some(),
        "the cause is what names the port"
    );
}
