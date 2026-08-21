//! What one embedded file is, and the group it belongs to.
//!
//! `label` is the one place this crate concedes anything to a runner:
//! refinery's parser wants `V{version}__{name}`, and spelling it out here
//! rather than deriving it there keeps the ledger's keys a thing you can read
//! rather than compute.

/// One `.sql` file, and the two names it goes by.
pub struct Embedded {
    /// The basename without `.sql`, which is what the directory calls it. Only
    /// the completeness test reads this.
    pub file: &'static str,
    /// What refinery calls it. Its parser requires `V{version}__{name}`, and
    /// the version has to be an integer, so `012_position_supply` becomes
    /// `V12__position_supply` — same order, refinery's spelling.
    pub label: &'static str,
    /// The whole file, sent as it stands. One statement per file, so nothing
    /// here has to be taken apart before it reaches a server.
    pub sql: &'static str,
}

/// One directory of schema, and where it is read from.
pub struct Source {
    /// Relative to this crate's manifest, which is what a test's cwd is.
    pub directory: &'static str,
    pub files: &'static [Embedded],
}
