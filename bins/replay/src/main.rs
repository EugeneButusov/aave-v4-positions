//! Replays one corpus against both read APIs and compares what they answer.
//!
//! Phase 2's gate in `docs/rust-migration.md`: the TypeScript service is the
//! oracle, both are pointed at the same ClickHouse and Postgres, and the only
//! transform between them is every key read as `snake_case`.
//!
//! **The fold must be frozen while this runs.** An indexer that advances
//! mid-walk changes the shares under both services, and a page compared across
//! that is a finding about the clock rather than about either service.

mod compare;
mod config;
mod corpus;
mod fold;
mod http;
mod json;
mod keys;
mod report;
mod run;

#[cfg(test)]
mod test_support;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match run::gate().await {
        Ok(report) => {
            print!("{}", report.text());

            if report.agreed() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprint!("{}", report::causes(&error));

            ExitCode::FAILURE
        }
    }
}
