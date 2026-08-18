//! Ordinals ascend across a migration set, and every id is shaped so that they
//! can be compared at all.

use crate::migration::Migration;

/// Rejects a set whose ordinals do not strictly ascend.
///
/// The array is itself the apply order, so this is not resolving an ambiguity
/// about what runs first. What it enforces is that the array agrees with the
/// directory listing a reader sees, and — being strict — that no two migrations
/// share an ordinal, which is what happens when two crates each reach for `002`
/// without either author seeing the other.
///
/// Call it from a test over the constant that assembles the union, once per
/// database. **Two lists, never one:** two databases are free to share an
/// ordinal, and checking them together would reject a set that is perfectly fine.
///
/// # Errors
///
/// [`Error::Unnamed`] if an id is not `NNN_snake_case`, and [`Error::OutOfOrder`]
/// if an ordinal does not exceed the one before it.
fn check_order(migrations: &[Migration]) -> Result<(), Error> {
    let mut previous: Option<(&'static str, &'static str)> = None;

    for migration in migrations {
        let Some(ordinal) = ordinal_of(migration.id()) else {
            return Err(Error::Unnamed { id: migration.id() });
        };

        if let Some((seen, before)) = previous
            && ordinal <= seen
        {
            return Err(Error::OutOfOrder {
                previous: before,
                id: migration.id(),
            });
        }

        previous = Some((ordinal, migration.id()));
    }

    Ok(())
}

/// The three-digit ordinal of an id shaped `NNN_snake_case`, or `None`.
///
/// ASCII throughout, and not `char::is_numeric`: these ids are filenames, and a
/// wider digit class would accept ones that no directory listing orders the way
/// the ordinal claims.
fn ordinal_of(id: &'static str) -> Option<&'static str> {
    let (ordinal, rest) = id.split_at_checked(3)?;
    if !ordinal.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    let name = rest.strip_prefix('_')?;
    let named = !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');

    named.then_some(ordinal)
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error("migration \"{id}\" must be named NNN_snake_case.sql, e.g. 001_spoke_events.sql")]
    Unnamed { id: &'static str },

    #[error(
        "migrations must be listed in ascending ordinal order: \"{id}\" follows \"{previous}\""
    )]
    OutOfOrder {
        previous: &'static str,
        id: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(id: &'static str) -> Migration {
        Migration::new(id, "SELECT 1")
    }

    #[test]
    fn accepts_ascending_ordinals() {
        check_order(&[
            at("001_spoke_events"),
            at("002_hub_assets"),
            at("010_prices"),
        ])
        .unwrap();
    }

    #[test]
    fn rejects_a_pair_that_does_not_ascend() {
        // Two crates each reaching for 002 without either author seeing the other,
        // and an insertion that puts the array out of step with the directory
        // listing.
        for pair in [
            [at("002_hub_assets"), at("002_prices")],
            [at("010_prices"), at("005_hub_assets")],
        ] {
            let rejected = check_order(&pair).unwrap_err();

            assert_eq!(
                rejected.to_string(),
                format!(
                    "migrations must be listed in ascending ordinal order: \"{}\" follows \"{}\"",
                    pair[1].id(),
                    pair[0].id()
                )
            );
        }
    }

    #[test]
    fn rejects_every_unorderable_name_shape() {
        for id in [
            "1_spoke_events",
            "spoke_events",
            "0001_spoke_events",
            "001-spoke-events",
            "001_Spoke",
            "001",
        ] {
            let rejected = check_order(&[at(id)]).unwrap_err();

            assert_eq!(
                rejected.to_string(),
                format!(
                    "migration \"{id}\" must be named NNN_snake_case.sql, \
                     e.g. 001_spoke_events.sql"
                )
            );
        }
    }
}
