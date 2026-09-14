//! Type-independent parsing into a flat, postorder node arena.
use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};
use serde_json::value::RawValue;

#[derive(Debug, PartialEq)]
pub enum Node {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<u32>),
    Object(Vec<(String, u32)>),
    Temporal(TemporalKind, String),
    Bytes(Vec<u8>),
}

#[derive(Debug, PartialEq)]
pub enum TemporalKind {
    LocalDate,
    LocalTime,
    LocalDateTime,
    OffsetDateTime,
}

#[derive(Debug)]
pub struct Plan {
    pub nodes: Vec<Node>,
    pub root: u32,
}

impl Plan {
    pub fn parse(input: &str) -> Result<Self, String> {
        let root: &RawValue = serde_json::from_str(input).map_err(|e| e.to_string())?;
        let mut plan = Self {
            nodes: Vec::new(),
            root: 0,
        };
        plan.root = plan.value(root, 0)?;
        Ok(plan)
    }

    fn value(&mut self, raw: &RawValue, depth: usize) -> Result<u32, String> {
        if depth > 512 {
            return Err("JSON nesting limit".into());
        }
        let input = raw.get();
        let node = match input.as_bytes()[0] {
            b'n' => Node::Null,
            b't' => Node::Bool(true),
            b'f' => Node::Bool(false),
            b'"' => Node::String(serde_json::from_str(input).map_err(|e| e.to_string())?),
            b'[' => {
                let children: Vec<&RawValue> =
                    serde_json::from_str(input).map_err(|e| e.to_string())?;
                let mut ids = Vec::with_capacity(children.len());
                for child in children {
                    ids.push(self.value(child, depth + 1)?);
                }
                Node::Array(ids)
            }
            b'{' => {
                let fields: Fields<'_> = serde_json::from_str(input).map_err(|e| e.to_string())?;
                let mut ids = Vec::with_capacity(fields.0.len());
                for (key, value) in fields.0 {
                    ids.push((key, self.value(value, depth + 1)?));
                }
                Node::Object(ids)
            }
            _ if input.bytes().any(|b| matches!(b, b'.' | b'e' | b'E')) => {
                let value: f64 = input.parse().map_err(|_| "invalid JSON Float")?;
                if !value.is_finite() {
                    return Err("JSON Float must be finite".into());
                }
                Node::Float(value)
            }
            _ => Node::Int(
                input
                    .parse()
                    .map_err(|_| "JSON integer is outside the i64 range")?,
            ),
        };
        let id = u32::try_from(self.nodes.len()).map_err(|_| "JSON node count overflow")?;
        self.nodes.push(node);
        Ok(id)
    }
}

struct Fields<'a>(BTreeMap<String, &'a RawValue>);

impl<'de> Deserialize<'de> for Fields<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FieldsVisitor;
        impl<'de> Visitor<'de> for FieldsVisitor {
            type Value = Fields<'de>;
            fn expecting(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str("a JSON object with unique keys")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut fields = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, &'de RawValue>()? {
                    if fields.contains_key(&key) {
                        return Err(de::Error::custom(format!(
                            "duplicate JSON object key {key:?}"
                        )));
                    }
                    fields.insert(key, value);
                }
                Ok(Fields(fields))
            }
        }
        deserializer.deserialize_map(FieldsVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_spelling_preserves_int_and_float_identity() {
        for (input, expected) in [
            ("1", Node::Int(1)),
            ("-0", Node::Int(0)),
            ("1.0", Node::Float(1.0)),
            ("1e0", Node::Float(1.0)),
            ("9223372036854775807", Node::Int(i64::MAX)),
            ("-9223372036854775808", Node::Int(i64::MIN)),
        ] {
            assert_eq!(Plan::parse(input).unwrap().nodes, [expected]);
        }
        for input in [
            "9223372036854775808",
            "-9223372036854775809",
            "18446744073709551616",
        ] {
            assert_eq!(
                Plan::parse(input).unwrap_err(),
                "JSON integer is outside the i64 range"
            );
        }
        assert_eq!(
            Plan::parse("1e9999").unwrap_err(),
            "JSON Float must be finite"
        );
    }
    #[test]
    fn postorder_graph_has_sorted_unique_decoded_keys() {
        let plan = Plan::parse(r#"{"z":[],"a":[true,null,"\ud83d\ude00"]}"#).unwrap();
        assert_eq!(plan.root, 5);
        assert_eq!(
            plan.nodes,
            [
                Node::Bool(true),
                Node::Null,
                Node::String("😀".into()),
                Node::Array(alloc::vec![0, 1, 2]),
                Node::Array(Vec::new()),
                Node::Object(alloc::vec![("a".into(), 3), ("z".into(), 4)])
            ]
        );
        assert!(
            Plan::parse(r#"{"a":1,"\u0061":2}"#)
                .unwrap_err()
                .contains("duplicate JSON object key")
        );
    }
    #[test]
    fn syntax_is_validated_before_materialization() {
        for input in [
            "",
            "null true",
            "[1,]",
            "01",
            "{\"x\":}",
            "\"\\ud800\"",
            "NaN",
            "Infinity",
        ] {
            assert!(Plan::parse(input).is_err(), "{input}");
        }
    }
}
