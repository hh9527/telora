//! Portable data graph. No source text, parser handles, or host addresses.
use serde::{Deserialize, Serialize};
use telora_core::data_plan::{DataPlanNodeKind as K, DataScalar as S, ValidatedDataPlan};

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
    pub fn from_plan(plan: &ValidatedDataPlan) -> Result<Self, String> {
        let id =
            |index: usize| u32::try_from(index).map_err(|_| "Wasm: data index overflow".to_owned());
        let nodes = plan
            .nodes()
            .iter()
            .map(|node| {
                Ok(Node {
                    origin: plan.compact(node.location).0,
                    value: match &node.kind {
                        K::Scalar(scalar) => match scalar {
                            S::Int(value) => Value::Int(value.to_string()),
                            S::Float(value) => Value::Float(*value),
                            S::String(value) => Value::String(value.clone()),
                            S::Bytes(value) => Value::Bytes(value.clone()),
                            S::Null => Value::Null,
                            S::Bool(value) => Value::Bool(*value),
                            S::Temporal { kind, value } => Value::Temporal {
                                variant: kind.variant().into(),
                                value: value.clone(),
                            },
                        },
                        K::Array(items) => Value::Array(
                            items
                                .iter()
                                .map(|item| id(item.index()))
                                .collect::<Result<_, _>>()?,
                        ),
                        K::Object(fields) => Value::Object(
                            fields
                                .iter()
                                .map(|(name, field)| {
                                    Ok(Field {
                                        name: name.clone(),
                                        origin: plan.compact(field.key_location).0,
                                        value: id(field.value.index())?,
                                    })
                                })
                                .collect::<Result<_, String>>()?,
                        ),
                    },
                })
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            root: id(plan.root_node().ok_or("Wasm: missing data root")?.index())?,
            nodes,
        })
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
