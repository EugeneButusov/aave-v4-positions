use crate::statements::split_statements;

/// One migration file: the id it is known by, and the SQL it carries.
///
/// The id is the file's basename without `.sql`, written out beside the
/// `include_str!` because a macro cannot hand back a string literal's own
/// filename. A test compares the two, so they cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Migration {
    id: &'static str,
    sql: &'static str,
}

impl Migration {
    /// `const` so a crate's whole set can be a `const MIGRATIONS: &[Migration]`.
    #[must_use]
    pub const fn new(id: &'static str, sql: &'static str) -> Self {
        Self { id, sql }
    }

    /// The file's basename without `.sql`: `NNN_snake_case`.
    #[must_use]
    pub const fn id(&self) -> &'static str {
        self.id
    }

    /// The statements in the file, in file order, each with its `;` removed.
    ///
    /// A file holding several has to be taken apart before it is sent, because
    /// ClickHouse's HTTP interface refuses multi-statement requests outright —
    /// `Multi-statements are not allowed`.
    ///
    /// **The terminator is `;` because these files are written to be pasted.** A
    /// migration you cannot drop into a SQL console and run is one you cannot
    /// debug when it matters, and a file that separated statements with a comment
    /// marker could not be: the console parsed the whole buffer as one query and
    /// failed at the second statement, having created nothing.
    ///
    /// A statement still belongs in its own file unless it is meaningless apart
    /// from its neighbours. Three of the twenty hold more than one: the nine
    /// projections of a table are one change to read and one change to review,
    /// where a table and the view over it are two.
    pub fn statements(&self) -> impl Iterator<Item = &'static str> {
        split_statements(self.sql)
    }
}
