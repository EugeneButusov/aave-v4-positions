//! A JSON tree that keeps the order its keys arrived in.
//!
//! `serde_json::Value` holds an object in a `BTreeMap` unless `preserve_order`
//! is on, and that feature unifies across the workspace: asking for it here
//! reorders the keys in the document `bins/api` snapshots, and whether it is on
//! then depends on which packages the build happened to include. Key order is
//! part of what this binary compares, so the tree is local.

use std::fmt;

use serde::de::{Deserialize, Deserializer, Error, MapAccess, SeqAccess, Visitor};
use serde_json::Number;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Json {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Json>),

    /// In document order, which is the whole point of this type.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// `None` when the text is not JSON at all — a plain-text 500, or a proxy
    /// page in front of a service that is not up.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        serde_json::from_str(text).ok()
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(fields) => fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            _ => None,
        }
    }

    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    /// What a value is, for the one rule that compares nothing else.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "a boolean",
            Self::Number(_) => "a number",
            Self::String(_) => "a string",
            Self::Array(_) => "a list",
            Self::Object(_) => "an object",
        }
    }
}

impl fmt::Display for Json {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => out.write_str("null"),
            Self::Bool(value) => write!(out, "{value}"),
            Self::Number(value) => write!(out, "{value}"),
            Self::String(value) => write!(out, "{value:?}"),
            Self::Array(items) => {
                out.write_str("[")?;
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        out.write_str(",")?;
                    }
                    write!(out, "{item}")?;
                }
                out.write_str("]")
            }
            Self::Object(fields) => {
                out.write_str("{")?;
                for (at, (key, value)) in fields.iter().enumerate() {
                    if at > 0 {
                        out.write_str(",")?;
                    }
                    write!(out, "{key:?}:{value}")?;
                }
                out.write_str("}")
            }
        }
    }
}

impl<'de> Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(Tree)
    }
}

struct Tree;

impl<'de> Visitor<'de> for Tree {
    type Value = Json;

    fn expecting(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("any JSON value")
    }

    fn visit_unit<E: Error>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E: Error>(self, value: bool) -> Result<Json, E> {
        Ok(Json::Bool(value))
    }

    fn visit_i64<E: Error>(self, value: i64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_u64<E: Error>(self, value: u64) -> Result<Json, E> {
        Ok(Json::Number(value.into()))
    }

    fn visit_f64<E: Error>(self, value: f64) -> Result<Json, E> {
        Number::from_f64(value)
            .map(Json::Number)
            .ok_or_else(|| E::custom("a number JSON cannot hold"))
    }

    fn visit_str<E: Error>(self, value: &str) -> Result<Json, E> {
        Ok(Json::String(value.to_owned()))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Json, A::Error> {
        let mut read = Vec::new();

        while let Some(item) = items.next_element()? {
            read.push(item);
        }

        Ok(Json::Array(read))
    }

    /// `next_entry` yields in document order, so the `Vec` is the order.
    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<Json, A::Error> {
        let mut read = Vec::new();

        while let Some(entry) = fields.next_entry()? {
            read.push(entry);
        }

        Ok(Json::Object(read))
    }
}

#[cfg(test)]
mod tests {
    use super::Json;

    #[test]
    fn keeps_the_order_the_keys_arrived_in() {
        let Some(Json::Object(fields)) = Json::parse(r#"{"message":"x","error":"y","b":1,"a":2}"#)
        else {
            panic!("not an object");
        };
        let keys: Vec<&str> = fields.iter().map(|(key, _)| key.as_str()).collect();

        assert_eq!(keys, ["message", "error", "b", "a"]);
    }

    #[test]
    fn reads_every_shape_a_payload_here_carries() {
        let read = Json::parse(r#"{"a":null,"b":true,"c":-1,"d":1.5,"e":"x","f":[1,{"g":2}]}"#);

        assert_eq!(
            read.as_ref().map(ToString::to_string).as_deref(),
            Some(r#"{"a":null,"b":true,"c":-1,"d":1.5,"e":"x","f":[1,{"g":2}]}"#)
        );
    }

    #[test]
    fn a_body_that_is_not_json_does_not_read() {
        assert_eq!(Json::parse("Internal Server Error"), None);
        assert_eq!(Json::parse(""), None);
    }

    #[test]
    fn reaches_a_field_by_name_and_nowhere_else() {
        let read = Json::parse(r#"{"message":"no"}"#).unwrap();

        assert_eq!(read.get("message").and_then(Json::as_str), Some("no"));
        assert_eq!(read.get("error"), None);
        assert_eq!(Json::Null.get("message"), None);
    }

    #[test]
    fn names_what_a_value_is() {
        assert_eq!(Json::Null.kind(), "null");
        assert_eq!(Json::parse("1").unwrap().kind(), "a number");
        assert_eq!(Json::parse(r#""1""#).unwrap().kind(), "a string");
    }
}
