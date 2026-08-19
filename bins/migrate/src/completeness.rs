//! The embedded list and the directory it was embedded from still agree.
//!
//! No caller in the shipped binary, so the check lives with its tests and
//! nothing here is visible outside this module.

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};

    use crate::schema::{CLICKHOUSE, Embedded, POSTGRES, Source};

    /// Rejects a group that has drifted from the directory it was embedded from.
    ///
    /// `schema.rs` is written by hand, so nothing keeps it in step with the directory
    /// on its own — and refinery cannot help, because it only ever sees the list it
    /// is handed. Neither half can move without the other: a `.sql` file nobody added
    /// fails, and a list entry whose file was deleted or renamed fails too.
    ///
    /// Compares file *names*, not refinery's labels: `001_spoke_events.sql` is on
    /// disk, `V1__spoke_events` is what refinery calls it, and it is the first of
    /// those the directory can be asked about.
    ///
    /// # Errors
    ///
    /// [`Error::Unreadable`] if the directory cannot be listed — which matters, since
    /// a wrong path returning `Ok` would make the whole guarantee vacuous — and
    /// [`Error::NotEmbedded`] or [`Error::NotOnDisk`] for a disagreement either way.
    fn check_complete(files: &[Embedded], directory: &Path) -> Result<(), Error> {
        let unreadable = |source| Error::Unreadable {
            directory: directory.to_path_buf(),
            source,
        };

        let mut names: Vec<String> = Vec::new();
        for entry in fs::read_dir(directory).map_err(unreadable)? {
            let name = entry.map_err(unreadable)?.file_name();
            // A name that is not UTF-8 cannot be one of ours: the entries are Rust
            // string literals. Skipping it is the same answer as not matching `.sql`.
            let Some(name) = name.to_str() else { continue };

            if let Some(id) = name.strip_suffix(".sql") {
                names.push(id.to_owned());
            }
        }

        // `names` owns the strings `on_disk` borrows.
        let on_disk: BTreeSet<&str> = names.iter().map(String::as_str).collect();
        let embedded: BTreeSet<&str> = files.iter().map(|file| file.file).collect();

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
        #[error("{}/{id}.sql is not embedded by any entry in schema.rs", directory.display())]
        NotEmbedded { directory: PathBuf, id: String },

        #[error("schema.rs embeds \"{id}\", which is not a .sql file in {}", directory.display())]
        NotOnDisk { directory: PathBuf, id: String },

        #[error("could not read {}", directory.display())]
        Unreadable {
            directory: PathBuf,
            #[source]
            source: std::io::Error,
        },
    }

    /// A miniature of a group in `schema.rs`, pointed at this crate's fixtures.
    const FIXTURES: &[Embedded] = &[
        Embedded {
            file: "001_opens_with_prose",
            label: "V1__opens_with_prose",
            sql: include_str!("../fixtures/001_opens_with_prose.sql"),
        },
        Embedded {
            file: "002_second_file",
            label: "V2__second_file",
            sql: include_str!("../fixtures/002_second_file.sql"),
        },
    ];

    #[test]
    fn accepts_a_group_that_matches_its_directory() {
        check_complete(FIXTURES, Path::new("fixtures")).unwrap();
    }

    #[test]
    fn rejects_a_file_nobody_added_to_schema() {
        let missed = check_complete(&FIXTURES[..1], Path::new("fixtures")).unwrap_err();

        assert!(
            missed
                .to_string()
                .ends_with("/002_second_file.sql is not embedded by any entry in schema.rs"),
            "{missed}"
        );
    }

    #[test]
    fn rejects_an_entry_whose_file_is_gone() {
        // Every file on disk stays listed, so the only disagreement is the entry
        // pointing at a file that is not there.
        let renamed = [
            Embedded {
                file: "001_opens_with_prose",
                label: "V1__a",
                sql: "",
            },
            Embedded {
                file: "002_second_file",
                label: "V2__b",
                sql: "",
            },
            Embedded {
                file: "003_deleted",
                label: "V3__deleted",
                sql: "",
            },
        ];

        let gone = check_complete(&renamed, Path::new("fixtures")).unwrap_err();

        assert!(
            gone.to_string()
                .starts_with("schema.rs embeds \"003_deleted\", which is not a .sql file in "),
            "{gone}"
        );
    }

    #[test]
    fn says_which_directory_it_could_not_read() {
        // A wrong path answering Ok would make every other completeness test in
        // the workspace vacuous.
        let absent = Path::new("fixtures/no_such_directory");

        let error = check_complete(FIXTURES, absent).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("could not read {}", absent.display())
        );
    }

    /// The guard as the deployment actually needs it: every group in `schema.rs`
    /// against the directory it was embedded from. Per group, because each group
    /// is exactly one directory.
    #[test]
    fn every_group_matches_the_directory_it_was_embedded_from() {
        for Source { directory, files } in CLICKHOUSE.iter().chain(POSTGRES) {
            check_complete(files, Path::new(directory))
                .unwrap_or_else(|error| panic!("{directory}: {error}"));
        }
    }
}
