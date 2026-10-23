use std::panic::PanicHookInfo;

/// Sends a panic through the subscriber instead of to stderr.
///
/// The default hook writes plain text, and this process's stdout is one JSON
/// object per line: a panic lands in the middle of that stream with nothing
/// tying it to the request that caused it. `CatchPanicLayer` answers the caller,
/// but it runs after the hook has already written.
pub(crate) fn report(info: &PanicHookInfo<'_>) {
    said(
        payload(info),
        info.location().map(ToString::to_string).as_deref(),
    );
}

/// What `panic!` was given: a `String` when it was formatted, a `&str` when it
/// was a literal, and nothing else anything here creates.
fn payload<'a>(info: &'a PanicHookInfo<'a>) -> &'a str {
    info.payload()
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| info.payload().downcast_ref::<&'static str>().copied())
        .unwrap_or("a panic carrying no message")
}

/// Split from the hook so a case can drive it without installing one.
fn said(panic: &str, location: Option<&str>) {
    tracing::error!(panic, location, "a thread panicked");
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use tracing_subscriber::fmt::MakeWriter;

    use super::said;

    #[derive(Clone)]
    struct Captured(mpsc::Sender<Vec<u8>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.send(buffer.to_vec());

            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    fn logged(panic: &str, location: Option<&str>) -> String {
        let (sender, received) = mpsc::channel();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_writer(Captured(sender))
            .finish();

        tracing::subscriber::with_default(subscriber, || said(panic, location));

        received
            .try_iter()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .collect()
    }

    #[tokio::test]
    async fn a_panic_is_one_json_object_like_every_other_line() {
        let line = logged("it all went wrong", Some("src/thing.rs:12:5"));

        let parsed: serde_json::Value =
            serde_json::from_str(line.trim()).unwrap_or_else(|_| panic!("not JSON: {line}"));

        assert_eq!(parsed["level"], "ERROR");
        assert_eq!(parsed["fields"]["message"], "a thread panicked");
        assert_eq!(parsed["fields"]["panic"], "it all went wrong");
        assert_eq!(parsed["fields"]["location"], "src/thing.rs:12:5");
    }

    #[tokio::test]
    async fn a_panic_from_nowhere_still_renders() {
        let line = logged("a panic carrying no message", None);

        assert!(line.contains("a thread panicked"), "{line}");
    }

    /// Through the real hook, because [`PanicHookInfo`] cannot be built outside
    /// `std` and reading the payload is where the bug would be: `panic!("x")`
    /// carries a `&str` and `panic!("{x}")` a `String`, so downcasting to one
    /// of the two loses half the messages to the fallback.
    #[test]
    fn reads_a_panic_message_whether_it_was_formatted_or_a_literal() {
        for (panicking, expected) in [
            ((|| panic!("a literal")) as fn(), "a literal"),
            (|| panic!("a {} one", "formatted"), "a formatted one"),
            (
                || std::panic::panic_any(42_u8),
                "a panic carrying no message",
            ),
        ] {
            let (sender, received) = mpsc::channel();
            let subscriber = tracing_subscriber::fmt()
                .json()
                .with_writer(Captured(sender))
                .finish();

            // Restored, because the hook is process-wide and a sibling case
            // panicking in this window would otherwise report through it.
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(super::report));
            tracing::subscriber::with_default(subscriber, || {
                let _ = std::panic::catch_unwind(panicking);
            });
            std::panic::set_hook(previous);

            let line: String = received
                .try_iter()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .collect();

            assert!(line.contains(expected), "{line}");
            assert!(line.contains("panics.rs"), "the location: {line}");
        }
    }
}
