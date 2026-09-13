//! Borrow either input graph without allocating another graph or payload copy.
use crate::data_packet::{self, DataPacket};
use telora_core::data_plan::{
    DataField, DataNodeId, DataPlanNodeKind as K, DataScalar as S, ValidatedDataPlan,
};

#[derive(Clone, Copy)]
pub(crate) enum Graph<'a> {
    Parsed(&'a ValidatedDataPlan),
    Packet(&'a DataPacket),
}

pub(crate) struct Node<'a> {
    pub origin: [u32; 3],
    pub value: Value<'a>,
}

pub(crate) enum Value<'a> {
    Int(i64),
    Float(f64),
    String(&'a str),
    Bytes(&'a [u8]),
    Null,
    Bool(bool),
    Temporal { variant: &'a str, value: &'a str },
    Array(Children<'a>),
    Object(Fields<'a>),
}

pub(crate) enum Children<'a> {
    Parsed(std::slice::Iter<'a, DataNodeId>),
    Packet(std::slice::Iter<'a, u32>),
}

impl Iterator for Children<'_> {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        match self {
            Self::Parsed(iter) => iter.next().map(|id| id.index()),
            Self::Packet(iter) => iter.next().map(|id| *id as usize),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Parsed(iter) => iter.size_hint(),
            Self::Packet(iter) => iter.size_hint(),
        }
    }
}
impl ExactSizeIterator for Children<'_> {}

pub(crate) enum Fields<'a> {
    Parsed(std::collections::btree_map::Iter<'a, String, DataField>),
    Packet(std::slice::Iter<'a, data_packet::Field>),
}
pub(crate) struct Field<'a> {
    pub name: &'a str,
    pub origin: [u32; 3],
    pub value: usize,
}
impl<'a> Iterator for Fields<'a> {
    type Item = Field<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Parsed(iter) => iter.next().map(|(name, field)| Field {
                name,
                origin: origin(field.key_location),
                value: field.value.index(),
            }),
            Self::Packet(iter) => iter.next().map(|field| Field {
                name: &field.name,
                origin: field.origin,
                value: field.value as usize,
            }),
        }
    }
}
fn origin(loc: telora_core::Location) -> [u32; 3] {
    [loc.source.get(), loc.start, loc.end]
}

impl<'a> Graph<'a> {
    pub fn len(self) -> usize {
        match self {
            Self::Parsed(plan) => plan.nodes().len(),
            Self::Packet(plan) => plan.nodes.len(),
        }
    }
    pub fn root(self) -> Result<usize, String> {
        match self {
            Self::Parsed(plan) => plan
                .root_node()
                .map(|id| id.index())
                .ok_or("Wasm: missing data root".into()),
            Self::Packet(plan) => Ok(plan.root as usize),
        }
    }
    pub fn node(self, id: usize) -> Result<Node<'a>, String> {
        Ok(match self {
            Self::Parsed(plan) => {
                let node = plan.nodes().get(id).ok_or("Wasm: invalid data edge")?;
                Node {
                    origin: origin(node.location),
                    value: match &node.kind {
                        K::Scalar(scalar) => match scalar {
                            S::Int(n) => Value::Int(*n),
                            S::Float(n) => Value::Float(*n),
                            S::String(s) => Value::String(s),
                            S::Bytes(b) => Value::Bytes(b),
                            S::Null => Value::Null,
                            S::Bool(b) => Value::Bool(*b),
                            S::Temporal { kind, value } => Value::Temporal {
                                variant: kind.variant(),
                                value,
                            },
                        },
                        K::Array(items) => Value::Array(Children::Parsed(items.iter())),
                        K::Object(fields) => Value::Object(Fields::Parsed(fields.iter())),
                    },
                }
            }
            Self::Packet(plan) => {
                use data_packet::Value as P;
                let node = plan.nodes.get(id).ok_or("Wasm: invalid data edge")?;
                Node {
                    origin: node.origin,
                    value: match &node.value {
                        P::Int(n) => Value::Int(n.parse().map_err(|_| "Wasm: invalid data Int")?),
                        P::Float(n) => Value::Float(*n),
                        P::String(s) => Value::String(s),
                        P::Bytes(b) => Value::Bytes(b),
                        P::Null => Value::Null,
                        P::Bool(b) => Value::Bool(*b),
                        P::Temporal { variant, value } => Value::Temporal { variant, value },
                        P::Array(items) => Value::Array(Children::Packet(items.iter())),
                        P::Object(fields) => Value::Object(Fields::Packet(fields.iter())),
                    },
                }
            }
        })
    }
}
