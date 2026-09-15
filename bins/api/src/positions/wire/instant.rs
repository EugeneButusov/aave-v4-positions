//! The wire's one spelling of a timestamp.

use time::OffsetDateTime;
use time::format_description::BorrowedFormatItem;
use time::macros::format_description;

/// The four widths a subsecond may take, and nothing between them: the SI
/// buckets. A `timestamptz` holds microseconds and nothing finer, so only the
/// first three are reachable.
const WHOLE: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second]Z");
const MILLIS: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");
const MICROS: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:6]Z");
const NANOS: &[BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:9]Z");

/// The wire's spelling of an instant: RFC 3339, in UTC, at an SI width.
///
/// **Not `time`'s `Rfc3339`**, which trims *every* trailing zero: a
/// `timestamptz` of `…00.5+00` goes out as `…00.5Z`, a width strict parsers
/// refuse. The SI buckets above cannot produce one.
///
/// **It does not fix sorting**, which it reads as though it would: `.` is below
/// `Z`, so `…00.500Z` sorts before `…00Z` at any width. These are instants, not
/// sort keys.
///
/// UTC is forced rather than assumed — a non-zero offset would print `+02:00`
/// where every reader of this field expects `Z`.
///
/// # Errors
///
/// [`time::error::Format`], which needs an unrepresentable year to produce.
pub(crate) fn instant(at: OffsetDateTime) -> Result<String, time::error::Format> {
    let at = at.to_offset(time::UtcOffset::UTC);

    at.format(match at.nanosecond() {
        0 => WHOLE,
        nanos if nanos % 1_000_000 == 0 => MILLIS,
        nanos if nanos % 1_000 == 0 => MICROS,
        _ => NANOS,
    })
}

/// The same, from the Unix seconds the store values a page at.
///
/// # Errors
///
/// [`time::Error`] when the seconds are outside the range a date can hold, which
/// `as_of`'s bounds already exclude.
pub(crate) fn instant_at(seconds: u64) -> Result<String, time::Error> {
    // The same fault an out-of-range timestamp produces, a century early.
    let seconds = i64::try_from(seconds).map_err(|_| time::error::ConversionRange)?;

    Ok(instant(OffsetDateTime::from_unix_timestamp(seconds)?)?)
}

#[cfg(test)]
mod tests {
    //! The one spelling of a timestamp, and the two shapes it is not.

    use time::OffsetDateTime;

    use super::{instant, instant_at};

    #[test]
    fn writes_a_whole_second_without_a_fractional_part() {
        // Pinned so the width cannot drift back by accident.
        assert_eq!(
            instant_at(1_788_796_630).ok().as_deref(),
            Some("2026-09-07T15:57:10Z")
        );
    }

    #[test]
    fn pads_a_fraction_up_to_the_next_si_width() {
        // The whole point of the buckets. `time`'s `Rfc3339` writes `.5Z` here,
        // which a strict parser refuses and which sorts after a whole second in
        // the same second.
        let half = OffsetDateTime::from_unix_timestamp_nanos(1_785_000_000_500_000_000)
            .expect("a representable instant");

        assert_eq!(
            instant(half).ok().as_deref(),
            Some("2026-07-25T17:20:00.500Z")
        );
    }

    #[test]
    fn takes_each_si_width_and_nothing_between_them() {
        let at = |nanos: i128| {
            instant(
                OffsetDateTime::from_unix_timestamp_nanos(1_785_000_000_000_000_000 + nanos)
                    .expect("a representable instant"),
            )
            .ok()
            .unwrap_or_default()
        };

        for (nanos, expected) in [
            (0, "2026-07-25T17:20:00Z"),
            (1_000_000, "2026-07-25T17:20:00.001Z"),
            (221_000_000, "2026-07-25T17:20:00.221Z"),
            (221_456_000, "2026-07-25T17:20:00.221456Z"),
            (221_456_789, "2026-07-25T17:20:00.221456789Z"),
        ] {
            assert_eq!(at(nanos), expected, "{nanos} nanoseconds");
        }
    }

    #[test]
    fn keeps_the_microseconds_postgres_stored() {
        // Postgres holds six, and this is the field a caller compares against
        // a row.
        let at = OffsetDateTime::from_unix_timestamp_nanos(1_785_000_000_221_456_000)
            .expect("a representable instant");

        assert_eq!(
            instant(at).ok().as_deref(),
            Some("2026-07-25T17:20:00.221456Z")
        );
    }

    #[test]
    fn writes_zulu_rather_than_a_zero_offset() {
        let at = OffsetDateTime::from_unix_timestamp(1_785_000_000)
            .expect("a representable instant")
            .to_offset(time::UtcOffset::from_hms(2, 0, 0).expect("a real offset"));

        assert_eq!(instant(at).ok().as_deref(), Some("2026-07-25T17:20:00Z"));
    }

    #[test]
    fn refuses_an_instant_no_date_can_hold_rather_than_wrapping() {
        assert!(instant_at(u64::MAX).is_err());
    }
}
