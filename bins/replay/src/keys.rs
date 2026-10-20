//! The one transform the comparison applies before it compares.

use crate::json::Json;

/// Every key of the oracle's payload, recursively, read as `snake_case`.
///
/// Total: no per-key exceptions, which is what makes the two key sets agreeing
/// an assertion rather than a convention. A field that is missing, extra or
/// misspelled still fails.
///
/// # Errors
///
/// [`Collision`], when two keys of one object read as the same name. The
/// transform would drop one of them and the comparison would pass without ever
/// seeing it.
pub(crate) fn snake_case(value: Json) -> Result<Json, Collision> {
    match value {
        Json::Object(fields) => object(fields),
        Json::Array(items) => items
            .into_iter()
            .map(snake_case)
            .collect::<Result<_, _>>()
            .map(Json::Array),
        scalar => Ok(scalar),
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{first:?} and {second:?} both read as {read:?}")]
pub(crate) struct Collision {
    first: String,
    second: String,
    read: String,
}

fn object(fields: Vec<(String, Json)>) -> Result<Json, Collision> {
    let mut read: Vec<(String, Json)> = Vec::with_capacity(fields.len());
    // The key each name was read from, so a collision can name both spellings
    // rather than the one they share.
    let mut sources: Vec<(String, String)> = Vec::with_capacity(fields.len());

    for (key, value) in fields {
        let name = snake(&key);

        if let Some((_, first)) = sources.iter().find(|(name_read, _)| *name_read == name) {
            return Err(Collision {
                first: first.clone(),
                second: key,
                read: name,
            });
        }

        sources.push((name.clone(), key));
        read.push((name, snake_case(value)?));
    }

    Ok(Json::Object(read))
}

/// An underscore before an upper-case letter that ends a word, and before one
/// that ends a run of them: `priceUsd` and `priceUSD` both read `price_usd`,
/// where a rule of "underscore before every capital" gives `price_u_s_d`.
fn snake(key: &str) -> String {
    let letters: Vec<char> = key.chars().collect();
    let mut read = String::with_capacity(key.len() + 4);

    for (at, letter) in letters.iter().enumerate() {
        if letter.is_ascii_uppercase() {
            let after_word =
                at > 0 && (letters[at - 1].is_lowercase() || letters[at - 1].is_numeric());
            let ends_a_run = at > 0
                && letters[at - 1].is_ascii_uppercase()
                && letters.get(at + 1).is_some_and(|next| next.is_lowercase());

            if after_word || ends_a_run {
                read.push('_');
            }

            read.push(letter.to_ascii_lowercase());
        } else {
            read.push(*letter);
        }
    }

    read
}

#[cfg(test)]
mod tests {
    use crate::json::Json;

    use super::{snake, snake_case};

    fn read(text: &str) -> String {
        snake_case(Json::parse(text).unwrap()).unwrap().to_string()
    }

    #[test]
    fn reads_the_keys_this_payload_actually_carries() {
        assert_eq!(snake("suppliedShares"), "supplied_shares");
        assert_eq!(snake("nextCursor"), "next_cursor");
        assert_eq!(snake("premiumOffsetRay"), "premium_offset_ray");
        assert_eq!(snake("uptimeSeconds"), "uptime_seconds");
        assert_eq!(snake("statusCode"), "status_code");
    }

    /// A run of capitals is one word. `price_u_s_d` would be a key neither
    /// service serves, so every comparison under it would fail.
    #[test]
    fn a_run_of_capitals_is_one_word() {
        assert_eq!(snake("priceUsd"), "price_usd");
        assert_eq!(snake("priceUSD"), "price_usd");
        assert_eq!(snake("totalDebtUSD"), "total_debt_usd");
        assert_eq!(snake("USDPrice"), "usd_price");
    }

    #[test]
    fn a_key_already_read_that_way_is_left_alone() {
        assert_eq!(snake("supplied_shares"), "supplied_shares");
        assert_eq!(snake("user"), "user");
        assert_eq!(snake(""), "");
    }

    #[test]
    fn reaches_every_key_through_objects_and_arrays() {
        assert_eq!(
            read(r#"{"nextCursor":null,"items":[{"chainId":1,"value":{"priceUsd":"1.0"}}]}"#),
            r#"{"next_cursor":null,"items":[{"chain_id":1,"value":{"price_usd":"1.0"}}]}"#
        );
    }

    /// Order survives, because the comparison asserts on it.
    #[test]
    fn keeps_the_order_the_payload_arrived_in() {
        assert_eq!(
            read(r#"{"message":"x","error":"y","statusCode":400}"#),
            r#"{"message":"x","error":"y","status_code":400}"#
        );
    }

    #[test]
    fn refuses_two_keys_that_read_as_one() {
        let collision =
            snake_case(Json::parse(r#"{"priceUsd":"1","price_usd":"2"}"#).unwrap()).unwrap_err();

        assert_eq!(
            collision.to_string(),
            r#""priceUsd" and "price_usd" both read as "price_usd""#
        );
    }
}
