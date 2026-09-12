//! Where the variables come from, which is a deployment's decision and not a
//! `main`'s.
//!
//! The process environment is always read. Whether a local `.env` fills what it
//! leaves out is what changes between a checkout and a container, and
//! [`Source::from_env`] is where that is settled — from a variable, so the same
//! binary does both.

use std::collections::HashMap;

/// Where a process's variables are read from.
///
/// **Owned here rather than by a `main` that mutates the environment before
/// anything reads it.** A binary asks its config for a `Config`; what that is
/// built out of is this crate's answer, and a deployment can change it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The process environment alone, which is what a deployment injects. A
    /// `.env` sitting on disk is not consulted.
    Process,
    /// The process environment, with a `.env` beside the working directory
    /// filling in what it leaves out — never replacing it. An exported variable
    /// beats the file, which is what makes the file a default rather than an
    /// override.
    ProcessAndFile,
}

impl Source {
    /// What `APP_ENV` asks for.
    ///
    /// **The one variable this crate will not read from a file**, because it
    /// decides whether there is a file to read. It comes from the process
    /// environment or not at all.
    ///
    /// **Where the TypeScript is deliberately not matched.** Nest skips the file
    /// only when `NODE_ENV` is `test` — a production deployment reads it, and
    /// would honour one that reached the image. Nothing ships one here:
    /// `.dockerignore` excludes `.env` and `.env.*`. This moves that guarantee
    /// out of a packaging rule and into the process, so a file arriving by some
    /// route nobody predicted still changes nothing.
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
            // Ignoring the error, because the usual one is that there is no
            // file. A malformed one is silent too, which is a local-development
            // problem with a local-development symptom.
            Self::ProcessAndFile => match dotenvy::dotenv_iter() {
                Ok(file) => merge(process, file.flatten()),
                Err(_) => process,
            },
        }
    }

    /// Absent is local, which is the default the TypeScript's `NODE_ENV` also
    /// carries and what the repository's `cp .env.example .env` expects.
    ///
    /// A spelling nobody recognises reads the file as well. That is the safe
    /// side of the mistake: a deployment that fat-fingers this has no file for
    /// the fallback to find, while a developer who does keeps working.
    fn pick(value: Option<&str>) -> Self {
        match value {
            Some("production" | "test") => Self::Process,
            _ => Self::ProcessAndFile,
        }
    }
}

/// The file fills gaps and nothing else, which is the whole contract a
/// deployment is owed.
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
    //! The two pure halves of the decision: which source a deployment asked
    //! for, and what merging owes the process that already has its variables.
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> HashMap<String, String> {
        items
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn a_deployments_variables_beat_the_file() {
        // The guarantee a deployment is owed, and the reason `merge` inserts
        // rather than extends: an injected variable is never replaced by a file
        // that reached the image by some route nobody predicted.
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
        // Including a spelling nobody recognises, which is the safe side of
        // that mistake: a deployment fat-fingering it has no file to find, and
        // a developer who does keeps working.
        for value in [None, Some("development"), Some("prodution"), Some("")] {
            assert_eq!(Source::pick(value), Source::ProcessAndFile, "{value:?}");
        }
    }
}
