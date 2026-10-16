//! Rendering an error with everything it wrapped.
//!
//! Shared by two unrelated callers: the `error` field of a failed readiness
//! check, and the line `bins/api` logs when a request fails.

/// The error's own message followed by its causes, skipping any already
/// spelled out by the one above it.
///
/// A *cause* chain, not a call stack — `source()` walks what each error
/// wrapped, so nothing here captures a backtrace.
///
/// Two conventions live side by side in the ecosystem: `thiserror` messages
/// here name only their own stage and leave the cause to `source()`, while
/// `deadpool`'s interpolate theirs. Walking a mixed chain naively prints the
/// bottom half twice — measured against the real refusal, which came out as
/// "error connecting to server: error connecting to server: Connection
/// refused". A containment test reconciles both without either side having to
/// know about the other.
pub fn error_message_with_causes(error: &dyn std::error::Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();

    while let Some(next) = cause {
        let text = next.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        cause = next.source();
    }

    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, thiserror::Error)]
    #[error("connection refused")]
    struct Refused;

    /// `thiserror`'s convention: the message names this stage only.
    #[derive(Debug, thiserror::Error)]
    #[error("could not check out a connection")]
    struct Unavailable(#[source] Refused);

    /// `deadpool`'s convention: the message already contains the source's.
    #[derive(Debug, thiserror::Error)]
    #[error("could not create an object: {0}")]
    struct Verbose(#[source] Refused);

    #[test]
    fn carries_the_cause_as_well_as_the_head() {
        assert_eq!(
            error_message_with_causes(&Unavailable(Refused)),
            "could not check out a connection: connection refused"
        );
    }

    #[test]
    fn says_a_cause_once_even_when_the_message_already_spells_it_out() {
        // Measured: walking this chain naively produced "error connecting to
        // server: error connecting to server: Connection refused", which reads
        // as two failures rather than one.
        assert_eq!(
            error_message_with_causes(&Verbose(Refused)),
            "could not create an object: connection refused"
        );
    }
}
