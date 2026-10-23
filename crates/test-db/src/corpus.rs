//! The Postgres corpus, as one string a schema can be built from.

/// Replayed rather than run through refinery: the ledger refinery keeps is a
/// deployment's record, and a schema built for one case has nothing to record.
pub(crate) fn sql() -> String {
    migrations::POSTGRES
        .iter()
        .flat_map(|source| source.files)
        .map(|embedded| embedded.sql)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    /// What lets the join above be a newline. A file that left its last
    /// statement unterminated would be merged into the next file's first one,
    /// and the replay would fail somewhere else entirely.
    ///
    /// Whether every file on disk is embedded at all is
    /// `migrations::completeness`'s question, not this one.
    #[test]
    fn every_file_terminates_its_last_statement() {
        for source in migrations::POSTGRES {
            for embedded in source.files {
                assert!(
                    embedded.sql.trim_end().ends_with(';'),
                    "{} does not end in a semicolon",
                    embedded.file
                );
            }
        }
    }
}
