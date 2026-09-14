//! Deserialize Host contracts from the same LLW JSON parser used for data.
//! Serde maps the validated nodes to Rust types; it does not parse text here.
use crate::{
    SourceDatabase,
    data_plan::{self, DataNodeId, DataPlanNodeKind, DataScalar, Format, ValidatedDataPlan},
};
use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use core::fmt;
use serde::de::{
    self, DeserializeOwned, IntoDeserializer, Visitor,
    value::{MapDeserializer, SeqDeserializer},
};

#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl core::error::Error for Error {}
impl de::Error for Error {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

pub fn from_slice<T: DeserializeOwned>(input: &[u8]) -> Result<T, Error> {
    from_str(core::str::from_utf8(input).map_err(|e| Error(e.to_string()))?)
}

pub fn from_str<T: DeserializeOwned>(input: &str) -> Result<T, Error> {
    let mut sources = SourceDatabase::default();
    let source = sources
        .try_add("<json>", input)
        .map_err(|e| Error(e.to_string()))?;
    let plan = data_plan::parse_registered(&sources, source, Format::Json).map_err(|errors| {
        Error(
            errors
                .iter()
                .map(|e| sources.render(e))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    })?;
    data_plan::enforce_limits(&plan, crate::DataLimits::default(), input.len()).map_err(Error)?;
    T::deserialize(Node {
        plan: &plan,
        id: plan.root_node().expect("parsed root"),
    })
}

#[derive(Clone, Copy)]
struct Node<'a> {
    plan: &'a ValidatedDataPlan,
    id: DataNodeId,
}

impl<'a> Node<'a> {
    fn child(self, id: DataNodeId) -> Self {
        Self {
            plan: self.plan,
            id,
        }
    }
    fn kind(self) -> &'a DataPlanNodeKind {
        &self.plan.nodes()[self.id.index()].kind
    }
}

impl<'de> IntoDeserializer<'de, Error> for Node<'_> {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self {
        self
    }
}

impl<'de> de::Deserializer<'de> for Node<'_> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.kind() {
            DataPlanNodeKind::Scalar(value) => match value {
                DataScalar::Null => visitor.visit_unit(),
                DataScalar::Bool(value) => visitor.visit_bool(*value),
                DataScalar::Int(value) => visitor.visit_i64(*value),
                DataScalar::Float(value) => visitor.visit_f64(*value),
                DataScalar::String(value) => visitor.visit_str(value),
                _ => Err(Error("non-JSON scalar in JSON document".into())),
            },
            DataPlanNodeKind::Array(items) => de::Deserializer::deserialize_any(
                SeqDeserializer::new(items.iter().map(|id| self.child(*id))),
                visitor,
            ),
            DataPlanNodeKind::Object(fields) => de::Deserializer::deserialize_any(
                MapDeserializer::new(
                    fields
                        .iter()
                        .map(|(key, field)| (key.as_str(), self.child(field.value))),
                ),
                visitor,
            ),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if matches!(self.kind(), DataPlanNodeKind::Scalar(DataScalar::Null)) {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self.kind() {
            DataPlanNodeKind::Scalar(DataScalar::String(value)) => {
                visitor.visit_enum(value.as_str().into_deserializer())
            }
            DataPlanNodeKind::Object(fields) if fields.len() == 1 => {
                visitor.visit_enum(de::value::MapAccessDeserializer::new(MapDeserializer::new(
                    fields
                        .iter()
                        .map(|(key, field)| (key.as_str(), self.child(field.value))),
                )))
            }
            _ => Err(Error(
                "expected an enum name or a single-variant JSON object".into(),
            )),
        }
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_contracts_use_llw_numbers_strings_and_container_boundaries() {
        let value: (Vec<i64>, Option<String>, bool) =
            from_str(r#"[[1,-2],"\uD83D\uDE00",true]"#).unwrap();
        assert_eq!(value, (vec![1, -2], Some("😀".into()), true));
        assert!(from_str::<(i64,)>("[1, 2]").is_err());
        assert!(from_str::<serde_json::Value>(r#"{"x":1,"x":2}"#).is_err());
        assert!(from_str::<serde_json::Value>("9223372036854775808").is_err());
        assert!(from_str::<serde_json::Value>("1e999").is_err());
        assert!(from_slice::<String>(&[0xff]).is_err());
    }
}
