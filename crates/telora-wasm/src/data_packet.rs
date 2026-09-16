//! Portable data graph. No source text, parser handles, or host addresses.
use serde::{Deserialize, Serialize};
use telora_core::{SourceDatabase, data_plan::ParsedData};
use crate::data_view::{Graph, Value as V};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DataPacket {
    pub root: u32,
    pub nodes: Vec<Node>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub origin: [u32; 3],
    pub value: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub origin: [u32; 3],
    pub value: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Value {
    // Decimal text preserves the entire i64 range in browser JSON transport.
    Int(String),
    Float(f64),
    String(String),
    Bytes(Vec<u8>),
    Null,
    Bool(bool),
    Temporal { variant: String, value: String },
    Array(Vec<u32>),
    Object(Vec<Field>),
}


impl Value {
    fn children(&self) -> impl DoubleEndedIterator<Item = u32> + '_ {
        let items = match self {
            Self::Array(items) => items.as_slice(),
            _ => &[],
        };
        let fields = match self {
            Self::Object(fields) => fields.as_slice(),
            _ => &[],
        };
        items
            .iter()
            .copied()
            .chain(fields.iter().map(|field| field.value))
    }
}

impl DataPacket {
    pub fn from_plan(plan: &ParsedData, sources: &SourceDatabase) -> Result<Self, String> {
        let graph = Graph::parsed(plan, sources)?;
        let id = |index: usize| u32::try_from(index).map_err(|_| "Wasm: data index overflow".to_owned());
        let nodes = (0..graph.len()).map(|index| {
            let node = graph.node(index)?;
            let value = match node.value {
                V::Int(n) => Value::Int(n.to_string()), V::Float(n) => Value::Float(n),
                V::String(s) => Value::String(s.into()), V::Bytes(b) => Value::Bytes(b.into()),
                V::Null => Value::Null, V::Bool(b) => Value::Bool(b),
                V::Temporal { variant, value } => Value::Temporal { variant: variant.into(), value: value.into() },
                V::Array(items) => Value::Array(items.map(id).collect::<Result<_, _>>()?),
                V::Object(fields) => Value::Object(fields.map(|field| Ok(Field {
                    name: field.name.into(), origin: field.origin, value: id(field.value)?,
                })).collect::<Result<_, String>>()?),
            };
            Ok(Node { origin: node.origin, value })
        }).collect::<Result<_, String>>()?;
        Ok(Self { root: id(graph.root()?)?, nodes })
    }

    /// Validate before materialization, including unreachable nodes. Deserializing
    /// a packet does not grant it the trusted status of a parsed source plan.
    pub fn validate(&self, manifest: &crate::artifact::Manifest) -> Result<(), String> {
        if self.root as usize >= self.nodes.len() {
            return Err("Wasm: invalid data root".into());
        }
        let origin = |loc: [u32; 3]| {
            let loc = telora_core::source::CompactLoc(loc);
            if loc.start() > loc.end() || !manifest.sources.iter().any(|source| source.id == loc.source()) {
                Err("Wasm: invalid data origin".to_owned())
            } else {
                Ok(())
            }
        };
        for node in &self.nodes {
            origin(node.origin)?;
            match &node.value {
                Value::Int(text) => {
                    text.parse::<i64>().map_err(|_| "Wasm: invalid data Int")?;
                }
                Value::Float(value) if !value.is_finite() => {
                    return Err("Wasm: non-finite data Float".into());
                }
                Value::Temporal { variant, .. }
                    if !matches!(
                        variant.as_str(),
                        "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
                    ) =>
                {
                    return Err("Wasm: invalid temporal variant".into());
                }
                Value::Object(fields) => {
                    if fields.windows(2).any(|pair| pair[0].name >= pair[1].name) {
                        return Err("Wasm: data keys must be unique and sorted".into());
                    }
                    for field in fields {
                        origin(field.origin)?;
                    }
                }
                _ => {}
            };
            if node
                .value
                .children()
                .any(|id| id as usize >= self.nodes.len())
            {
                return Err("Wasm: invalid data edge".into());
            }
        }
        let mut state = vec![0u8; self.nodes.len()];
        for root in 0..self.nodes.len() {
            let mut stack = vec![(root, false, 0usize)];
            while let Some((id, done, depth)) = stack.pop() {
                if done {
                    state[id] = 2;
                    continue;
                }
                if depth > 512 {
                    return Err("Wasm: data nesting limit".into());
                }
                match state[id] {
                    1 => return Err("Wasm: cyclic data packet".into()),
                    2 => continue,
                    _ => {}
                }
                state[id] = 1;
                stack.push((id, true, depth));
                stack.extend(
                    self.nodes[id]
                        .value
                        .children()
                        .rev()
                        .map(|child| (child as usize, false, depth + 1)),
                );
            }
        }
        Ok(())
    }
}
