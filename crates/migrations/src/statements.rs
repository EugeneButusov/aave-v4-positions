/// Where the scan is when it meets a character. Only `Code` can end a statement.
enum Lexical {
    Code,
    Comment,
    Quoted(char),
}

/// Splits a migration file into the statements it holds, on `;`.
///
/// The terminator is dropped: the runner sends one statement per request, and
/// what it sends stays exactly what the file says minus that character.
///
/// Every item borrows from `sql`, so nothing is allocated.
pub(crate) fn split_statements(sql: &str) -> impl Iterator<Item = &str> {
    Statements { rest: Some(sql) }
}

struct Statements<'a> {
    rest: Option<&'a str>,
}

impl<'a> Iterator for Statements<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(rest) = self.rest.take() {
            let (section, remainder) = match terminator(rest) {
                Some(at) => (&rest[..at], Some(&rest[at + 1..])),
                // An unterminated tail. Every file here ends in `;`, so this is
                // usually a trailing comment — but dropping a statement someone
                // forgot to terminate is how a table goes missing at deploy time.
                None => (rest, None),
            };
            self.rest = remainder;

            let statement = section.trim();
            if holds_sql(statement) {
                return Some(statement);
            }
        }

        None
    }
}

/// The byte index of the first `;` in open code, if there is one.
///
/// It cannot simply be `find(';')`. Measured over the twenty files: 723 `--`
/// comments, 184 string literals, 215 backtick identifiers and 14 double-quoted
/// ones — with nineteen semicolons living inside that prose, across half the
/// files. Cutting on one produces two halves plausible enough to fail somewhere
/// far from the cause.
///
/// Deliberately not handled, because the corpus holds none of them and untested
/// code is worse than absent code: block comments, `''` and backslash escapes,
/// and dollar quoting. A test holds the corpus to that.
fn terminator(sql: &str) -> Option<usize> {
    let mut inside = Lexical::Code;
    let mut chars = sql.char_indices().peekable();

    while let Some((at, char)) = chars.next() {
        match inside {
            Lexical::Code => match char {
                '-' if chars.peek().is_some_and(|&(_, next)| next == '-') => {
                    inside = Lexical::Comment;
                }
                '\'' | '"' | '`' => inside = Lexical::Quoted(char),
                ';' => return Some(at),
                _ => {}
            },
            Lexical::Comment if char == '\n' => inside = Lexical::Code,
            Lexical::Quoted(quote) if char == quote => inside = Lexical::Code,
            _ => {}
        }
    }

    None
}

/// One line that is neither blank nor a comment.
///
/// A file opens with prose, so the text before its first `;` can be a comment
/// block that would otherwise be sent on its own — which ClickHouse rejects as an
/// empty query, from the migration runner, at deploy time.
fn holds_sql(section: &str) -> bool {
    section.lines().any(|line| {
        let line = line.trim();
        !line.is_empty() && !line.starts_with("--")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(sql: &str) -> Vec<&str> {
        split_statements(sql).collect()
    }

    #[test]
    fn takes_the_terminator_off() {
        // The runner sends one statement per request, so what it sends is the
        // file's text minus that character and nothing else.
        assert_eq!(
            split("CREATE TABLE a (x UInt8);"),
            ["CREATE TABLE a (x UInt8)"]
        );
    }

    #[test]
    fn accepts_a_lone_statement_that_was_never_terminated() {
        assert_eq!(
            split("CREATE TABLE a (x UInt8)"),
            ["CREATE TABLE a (x UInt8)"]
        );
    }

    #[test]
    fn splits_on_the_terminator_and_trims_each_side() {
        assert_eq!(
            split("CREATE TABLE a (x UInt8);\n\nCREATE VIEW b AS SELECT 1;\n"),
            ["CREATE TABLE a (x UInt8)", "CREATE VIEW b AS SELECT 1"]
        );
    }

    #[test]
    fn does_not_cut_on_a_semicolon_inside_quoted_or_commented_text() {
        // Nineteen of these live in the real corpus, every one in a comment.
        for sql in [
            "-- a comment; with a semicolon\nCREATE VIEW v AS SELECT 1",
            "CREATE VIEW v AS SELECT 'has ; inside' AS s",
            "CREATE VIEW v AS SELECT 1 AS `has ; inside`",
            "CREATE VIEW v AS SELECT 1 AS \"has ; inside\"",
        ] {
            assert_eq!(split(sql), [sql], "{sql}");
        }
    }

    #[test]
    fn drops_the_prose_a_file_opens_with() {
        // Otherwise it is sent as a query of its own and ClickHouse answers
        // "Empty query" — from the migration runner, at deploy time.
        assert!(split("-- what this file is\n-- and why\n").is_empty());
    }

    #[test]
    fn keeps_the_comment_that_introduces_a_statement_attached_to_it() {
        let sql = "-- why this view exists\nCREATE VIEW v AS SELECT 1";

        assert_eq!(split(&format!("{sql};")), [sql]);
    }

    #[test]
    fn is_unbothered_by_a_doubled_or_trailing_terminator() {
        assert_eq!(split(";;\nSELECT 1;;\n;"), ["SELECT 1"]);
    }
}
