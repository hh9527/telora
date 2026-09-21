//! Deserialize Host contracts from the same stateful JSON parser used for data.
//! Serde maps the validated nodes to Rust types; it does not parse text here.
use crate::{
    SourceDatabase,
    data_plan::DataNodeId,
    json::{self, JsonKind as DataPlanNodeKind, JsonPlan, text::ParseCtx},
};
use alloc::string::{String, ToString};
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
        .try_add_data("<json>", String::new())
        .map_err(|e| Error(e.to_string()))?;
    let (plan, ctx) = json::parse_structure(source, input, crate::DataLimits::default())
        .and_then(json::JsonStructure::validate)
        .map_err(|errors| {
            // Source indexing is needed only when rendering a diagnostic.
            match sources.replace_unreferenced_data(source, "<json>", input.into()) {
                Ok(()) => Error(
                    errors
                        .iter()
                        .map(|error| sources.render(error))
                        .collect::<alloc::vec::Vec<_>>()
                        .join("\n"),
                ),
                Err(location) => Error(location.to_string()),
            }
        })?;
    T::deserialize(Node {
        plan: &plan,
        ctx: &ctx,
        id: plan.root,
    })
}

#[derive(Clone, Copy)]
struct Node<'a> {
    plan: &'a JsonPlan,
    ctx: &'a ParseCtx<'a>,
    id: DataNodeId,
}

impl<'a> Node<'a> {
    fn child(self, id: DataNodeId) -> Self {
        Self {
            plan: self.plan,
            ctx: self.ctx,
            id,
        }
    }
    fn kind(self) -> &'a DataPlanNodeKind {
        &self.plan.nodes[self.id.index()].kind
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
            DataPlanNodeKind::Null => visitor.visit_unit(),
            DataPlanNodeKind::Bool(value) => visitor.visit_bool(*value),
            DataPlanNodeKind::Int(value) => visitor.visit_i64(*value),
            DataPlanNodeKind::Float(value) => visitor.visit_f64(*value),
            DataPlanNodeKind::String(value) => visitor.visit_str(self.ctx.text(value)),
            DataPlanNodeKind::Array(items) => de::Deserializer::deserialize_any(
                SeqDeserializer::new(items.iter().map(|id| self.child(*id))),
                visitor,
            ),
            DataPlanNodeKind::Object(fields) => de::Deserializer::deserialize_any(
                MapDeserializer::new(
                    fields
                        .iter()
                        .map(|(key, field)| (self.ctx.text(key), self.child(field.value))),
                ),
                visitor,
            ),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if matches!(self.kind(), DataPlanNodeKind::Null) {
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
            DataPlanNodeKind::String(value) => {
                visitor.visit_enum(self.ctx.text(value).into_deserializer())
            }
            DataPlanNodeKind::Object(fields) if fields.len() == 1 => {
                visitor.visit_enum(de::value::MapAccessDeserializer::new(MapDeserializer::new(
                    fields
                        .iter()
                        .map(|(key, field)| (self.ctx.text(key), self.child(field.value))),
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
    use alloc::vec::Vec;

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
