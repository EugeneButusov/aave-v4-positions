//! Where the variables come from, which is a deployment's decision and not a
//! `main`'s.
//!
//! The process environment is always read. Whether a local `.env` fills what it
//! leaves out is what changes between a checkout and a container, and
//! [`Source::from_env`] is where that is settled — from a variable, so the same
//! binary does both.

use std::collections::HashMap;

/// Where a process's variables are read from. Owned here rather than by a
/// `main` that mutates the environment before anything reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The process environment alone, which is what a deployment injects. A
    /// `.env` sitting on disk is not consulted.
    Process,
    /// The process environment, with a `.env` filling in what it leaves out and
    /// never replacing it — so the file is a default, not an override.
    ProcessAndFile,
}

impl Source {
    /// What `APP_ENV` asks for. The one variable this crate will not read from
    /// a file, because it decides whether there is a file to read.
    ///
    /// Deliberately stricter than the TypeScript, which skips the file only
    /// under `NODE_ENV=test`: the guarantee moves out of `.dockerignore` and
    /// into the process.
    #[must_use]
    pub fn from_env() -> Self {
        Self::pick(std::env::var("APP_ENV").ok().as_deref())
    }

    /// Every variable this process can see, by whichever route it allows.
    #[must_use]
    pub fn read(self) -> HashMap<String, String> {
        let process = std::env::vars().collect();

        match self {
            Self::Process => process,
            // The usual error is that there is no file. A malformed one is
            // silent too, which is a local problem with a local symptom.
            Self::ProcessAndFile => match dotenvy::dotenv_iter() {
                Ok(file) => merge(process, file.flatten()),
                Err(_) => process,
            },
        }
    }

    /// Absent is local, as the TypeScript's `NODE_ENV` also has it.
    ///
    /// An unrecognised spelling reads the file too, which is the safe side: a
    /// deployment that fat-fingers it has no file to find.
    fn pick(value: Option<&str>) -> Self {
        match value {
            Some("production" | "test") => Self::Process,
            _ => Self::ProcessAndFile,
        }
    }
}

/// The file fills gaps and nothing else.
fn merge(
    mut process: HashMap<String, String>,
    file: impl Iterator<Item = (String, String)>,
) -> HashMap<String, String> {
    for (key, value) in file {
        process.entry(key).or_insert(value);
    }
    process
}

#[cfg(test)]
mod tests {
    //! Which source a deployment asked for, and what merging leaves alone.
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> HashMap<String, String> {
        items
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn a_deployments_variables_beat_the_file() {
        // Why `merge` inserts rather than extends: an injected variable is
        // never replaced by a file that reached the image.
        let merged = merge(
            pairs(&[("API_PORT", "3000")]),
            pairs(&[("API_PORT", "9999")]).into_iter(),
        );

        assert_eq!(merged["API_PORT"], "3000");
    }

    #[test]
    fn the_file_fills_only_what_the_process_left_out() {
        let merged = merge(
            pairs(&[("API_PORT", "3000")]),
            pairs(&[("API_PORT", "9999"), ("LOG_LEVEL", "debug")]).into_iter(),
        );

        assert_eq!(merged["API_PORT"], "3000");
        assert_eq!(merged["LOG_LEVEL"], "debug", "the gap was not filled");
    }

    #[test]
    fn a_deployment_reads_no_file() {
        assert_eq!(Source::pick(Some("production")), Source::Process);
        assert_eq!(Source::pick(Some("test")), Source::Process);
    }

    #[test]
    fn anything_else_reads_one() {
        // Including an unrecognised spelling, the safe side of the mistake.
        for value in [None, Some("development"), Some("prodution"), Some("")] {
            assert_eq!(Source::pick(value), Source::ProcessAndFile, "{value:?}");
        }
    }
}
