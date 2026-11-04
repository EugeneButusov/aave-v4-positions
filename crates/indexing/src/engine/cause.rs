//! The message the loop puts on a line when a port refuses.

/// Every source joined on, so the cause is not lost. Spelled out rather than
/// linking `ops`, which pulls axum: this crate may not reach a socket.
pub(crate) fn describe(error: impl std::error::Error) -> String {
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
    use super::describe;

    #[derive(Debug, thiserror::Error)]
    #[error("connection refused")]
    struct Refused;

    #[derive(Debug, thiserror::Error)]
    #[error("the cursor read failed")]
    struct Opaque(#[source] Refused);

    #[derive(Debug, thiserror::Error)]
    #[error("the cursor read failed: {0}")]
    struct Quoting(#[source] Refused);

    #[test]
    fn joins_a_source_its_error_does_not_mention() {
        assert_eq!(
            describe(Opaque(Refused)),
            "the cursor read failed: connection refused"
        );
    }

    #[test]
    fn does_not_repeat_a_source_its_error_already_quotes() {
        assert_eq!(
            describe(Quoting(Refused)),
            "the cursor read failed: connection refused"
        );
    }
}
