//! What a run says when it is done.

use std::fmt::Write as _;

/// One request the two services did not answer the same way.
#[derive(Debug)]
pub(crate) struct Finding {
    pub(crate) label: String,
    pub(crate) target: String,
    pub(crate) detail: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct Report {
    compared: usize,
    findings: Vec<Finding>,

    /// Requests whose status matched and whose body nothing stronger could be
    /// held to — counted by reason, so a run says how much of itself was a
    /// real comparison.
    unasserted: Vec<(String, &'static str)>,
}

impl Report {
    pub(crate) fn compared(&mut self) {
        self.compared += 1;
    }

    pub(crate) fn found(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    pub(crate) fn unasserted(&mut self, label: &str, reason: &'static str) {
        self.unasserted.push((label.to_owned(), reason));
    }

    /// One walker's report into the whole run's, in the order the corpus put
    /// the listings rather than the order the walkers finished.
    pub(crate) fn absorb(&mut self, walked: Self) {
        self.compared += walked.compared;
        self.findings.extend(walked.findings);
        self.unasserted.extend(walked.unasserted);
    }

    pub(crate) fn agreed(&self) -> bool {
        self.findings.is_empty()
    }

    pub(crate) fn text(&self) -> String {
        let mut text = String::new();

        for finding in &self.findings {
            let _ = writeln!(text, "{} — {}", finding.label, finding.target);

            for detail in &finding.detail {
                let _ = writeln!(text, "    {detail}");
            }
        }

        if !self.unasserted.is_empty() {
            let _ = writeln!(text, "\nstatus only, body unasserted:");

            for (label, reason) in &self.unasserted {
                let _ = writeln!(text, "    {label}: {reason}");
            }
        }

        let _ = writeln!(
            text,
            "\n{} requests compared, {} findings",
            self.compared,
            self.findings.len()
        );

        text
    }
}

/// An error and everything under it, one line each.
///
/// The interesting half of a failure here is usually two or three levels down
/// — a connection refused under a URL under a case — and `Display` on the
/// outermost shows none of it.
pub(crate) fn causes(error: &dyn std::error::Error) -> String {
    let mut text = format!("replay: {error}\n");
    let mut cause = error.source();

    while let Some(reason) = cause {
        let _ = writeln!(text, "  caused by: {reason}");
        cause = reason.source();
    }

    text
}

#[cfg(test)]
mod tests {
    use super::{Finding, Report, causes};

    #[test]
    fn a_run_with_nothing_to_say_says_how_much_it_compared() {
        let mut report = Report::default();
        report.compared();
        report.compared();

        assert!(report.agreed());
        assert_eq!(report.text(), "\n2 requests compared, 0 findings\n");
    }

    #[test]
    fn a_finding_carries_the_target_and_every_detail() {
        let mut report = Report::default();
        report.compared();
        report.found(Finding {
            label: "0xaa every spoke".to_owned(),
            target: "/v1/chains/1/users/0xaa/positions".to_owned(),
            detail: vec![
                "items: 3 items against 2".to_owned(),
                "a: 1 against 2".to_owned(),
            ],
        });

        assert!(!report.agreed());
        assert_eq!(
            report.text(),
            "0xaa every spoke — /v1/chains/1/users/0xaa/positions\n    items: 3 items against 2\n    a: 1 against 2\n\n1 requests compared, 1 findings\n"
        );
    }

    #[test]
    fn an_error_is_printed_with_everything_under_it() {
        #[derive(Debug, thiserror::Error)]
        #[error("the socket refused")]
        struct Refused;

        #[derive(Debug, thiserror::Error)]
        #[error("http://localhost:3000 did not answer")]
        struct Silent(#[source] Refused);

        assert_eq!(
            causes(&Silent(Refused)),
            "replay: http://localhost:3000 did not answer\n  caused by: the socket refused\n"
        );
    }

    #[test]
    fn an_error_with_nothing_under_it_is_one_line() {
        #[derive(Debug, thiserror::Error)]
        #[error("chain 1 has no positions in this fold")]
        struct Empty;

        assert_eq!(
            causes(&Empty),
            "replay: chain 1 has no positions in this fold\n"
        );
    }

    #[test]
    fn merges_what_each_walker_saw_and_keeps_their_order() {
        let mut whole = Report::default();

        for label in ["first", "second"] {
            let mut part = Report::default();
            part.compared();
            part.found(Finding {
                label: label.to_owned(),
                target: "/target".to_owned(),
                detail: vec!["a: 1 against 2".to_owned()],
            });
            whole.absorb(part);
        }

        let text = whole.text();

        assert!(text.contains("2 requests compared, 2 findings"), "{text}");
        assert!(
            text.find("first").unwrap() < text.find("second").unwrap(),
            "{text}"
        );
    }

    /// A run that compared nothing but statuses should not read as a clean one.
    #[test]
    fn says_which_bodies_it_could_not_hold_to_anything() {
        let mut report = Report::default();
        report.compared();
        report.unasserted("a parameter neither serves", "the oracle named none");

        assert!(report.agreed());
        assert!(
            report
                .text()
                .contains("    a parameter neither serves: the oracle named none")
        );
    }
}
