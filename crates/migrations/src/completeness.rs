//! The embedded list and the directory it was embedded from still agree.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::migration::Migration;

/// Rejects a set that has drifted from the directory it was embedded from.
///
/// The list is written by hand, so nothing keeps it in step with the directory on
/// its own. Call this from a test beside the constant, passing the directory the
/// `include_str!` paths point at, and neither half can move without the other: a
/// `.sql` file nobody added to the list fails, and a list entry whose file was
/// deleted or renamed fails too.
///
/// # Errors
///
/// [`Error::Unreadable`] if the directory cannot be listed — which matters, since
/// a wrong path returning `Ok` would make the whole guarantee vacuous — and
/// [`Error::NotEmbedded`] or [`Error::NotOnDisk`] for a disagreement either way.
fn check_complete(migrations: &[Migration], directory: &Path) -> Result<(), Error> {
    let unreadable = |source| Error::Unreadable {
        directory: directory.to_path_buf(),
        source,
    };

    let mut names: Vec<String> = Vec::new();
    for entry in fs::read_dir(directory).map_err(unreadable)? {
        let name = entry.map_err(unreadable)?.file_name();
        // A name that is not UTF-8 cannot be one of ours: the ids are Rust string
        // literals. Skipping it is the same answer as not matching `.sql`.
        let Some(name) = name.to_str() else { continue };

        if let Some(id) = name.strip_suffix(".sql") {
            names.push(id.to_owned());
        }
    }

    // `names` owns the strings `on_disk` borrows.
    let on_disk: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let embedded: BTreeSet<&str> = migrations.iter().map(Migration::id).collect();

    if let Some(&id) = on_disk.difference(&embedded).next() {
        return Err(Error::NotEmbedded {
            directory: directory.to_path_buf(),
            id: id.to_owned(),
        });
    }
    if let Some(&id) = embedded.difference(&on_disk).next() {
        return Err(Error::NotOnDisk {
            directory: directory.to_path_buf(),
            id: id.to_owned(),
        });
    }

    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("{}/{id}.sql is not embedded by any migration in the list", directory.display())]
    NotEmbedded { directory: PathBuf, id: String },

    #[error("the list embeds \"{id}\", which is not a .sql file in {}", directory.display())]
    NotOnDisk { directory: PathBuf, id: String },

    #[error("could not read {}", directory.display())]
    Unreadable {
        directory: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A miniature of what a crate that owns tables declares, and of what
    /// `check_complete` is pointed at there.
    const FIXTURES: &[Migration] = &[
        Migration::new(
            "001_opens_with_prose",
            include_str!("../fixtures/001_opens_with_prose.sql"),
        ),
        Migration::new(
            "002_two_statements",
            include_str!("../fixtures/002_two_statements.sql"),
        ),
    ];

    #[test]
    fn accepts_a_list_that_matches_its_directory() {
        check_complete(FIXTURES, Path::new("fixtures")).unwrap();
    }

    #[test]
    fn rejects_a_file_nobody_added_to_the_list() {
        let missed = check_complete(&FIXTURES[..1], Path::new("fixtures")).unwrap_err();

        assert!(
            missed
                .to_string()
                .ends_with("/002_two_statements.sql is not embedded by any migration in the list"),
            "{missed}"
        );
    }

    #[test]
    fn rejects_a_list_entry_whose_file_is_gone() {
        // Every file on disk stays listed, so the only disagreement is the entry
        // pointing at a file that is not there.
        let renamed = [
            FIXTURES[0],
            FIXTURES[1],
            Migration::new("003_deleted", "SELECT 1"),
        ];

        let gone = check_complete(&renamed, Path::new("fixtures")).unwrap_err();

        assert!(
            gone.to_string()
                .starts_with("the list embeds \"003_deleted\", which is not a .sql file in "),
            "{gone}"
        );
    }

    #[test]
    fn says_which_directory_it_could_not_read() {
        // A wrong path answering Ok would make every other completeness test in the
        // workspace vacuous.
        let absent = Path::new("fixtures/no_such_directory");

        let error = check_complete(FIXTURES, absent).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("could not read {}", absent.display())
        );
    }
}
