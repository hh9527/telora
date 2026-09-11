//! Candidate layouts only: consumes sealed types, never a VM or runtime Val.
use crate::{
    mir::{SealedMir, TypeConstructor as T, TypeId, TypeOperation},
    type_image::TypeImage,
};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Shape {
    pub data_bytes: u64,
    pub data_alignment: u64,
    pub value_bytes: u64,
    pub value_alignment: u64,
    pub table: Option<&'static str>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum State {
    Known {
        shape: Shape,
    },
    Pending {
        reason: String,
        dependencies: Vec<usize>,
    },
    Template,
}
#[derive(Debug, Serialize)]
pub struct Member {
    pub name: String,
    pub type_id: Option<usize>,
    pub offset: Option<u64>,
}
#[derive(Debug, Serialize)]
pub struct Object {
    pub status: &'static str,
    pub bytes: Option<u64>,
    pub element_type: Option<usize>,
    pub element_stride: Option<u64>,
    pub members: Vec<Member>,
    pub reason: Option<String>,
}
#[derive(Debug, Serialize)]
pub struct Entry {
    pub type_id: usize,
    /// Actual MIR constructor, independent of the human-readable type name.
    pub constructor: &'static str,
    pub layout: State,
    pub object: Option<Object>,
    pub variants: Vec<Member>,
}
fn constructor_name(constructor: &T) -> &'static str {
    match constructor {
        T::Int => "Int",
        T::Float => "Float",
        T::String => "String",
        T::Bytes => "Bytes",
        T::Bool => "Bool",
        T::Never => "Never",
        T::Type => "Type",
        T::TypeOf => "TypeOf",
        T::Dyn => "Dyn",
        T::Option => "Option",
        T::Result => "Result",
        T::FoldControl => "FoldControl",
        T::PropertyTarget => "PropertyTarget",
        T::PropertyBound => "PropertyBound",
        T::Unchecked => "Unchecked",
        T::TypeFunction(_) => "TypeFunction",
        T::Nominal(_) => "Nominal",
        T::Native(_) => "Native",
        T::Tuple => "Tuple",
        T::Array => "Array",
        T::ArrayLiteral => "ArrayLiteral",
        T::TupleLiteral => "TupleLiteral",
        T::TypeList => "TypeList",
        T::Dict => "Dict",
        T::Function => "Function",
        T::Quantified(_) => "Quantified",
        T::Bound(_) => "Bound",
        T::Record(_) => "Record",
        T::Newtype => "Newtype",
        T::Enum(_) => "Enum",
        T::Meta => "Meta",
        T::Namespace(_) => "Namespace",
        T::Parameter(_) => "Parameter",
    }
}
impl Entry {
    pub fn id(&self) -> TypeId {
        TypeId(self.type_id as u32)
    }
}

fn align(n: u64, a: u64) -> Result<u64, String> {
    n.checked_add(a - 1)
        .map(|n| n / a * a)
        .ok_or_else(|| "candidate layout size overflow".into())
}
fn shape(bytes: u64, alignment: u64, table: Option<&'static str>) -> Result<State, String> {
    Ok(State::Known {
        shape: Shape {
            data_bytes: bytes,
            data_alignment: alignment,
            value_bytes: align(bytes, 8)?
                .checked_add(16)
                .ok_or("candidate layout size overflow")?,
            value_alignment: 8,
            table,
        },
    })
}
fn pending(reason: &str, dependencies: Vec<usize>) -> State {
    State::Pending {
        reason: reason.into(),
        dependencies,
    }
}

struct Builder<'a> {
    image: &'a TypeImage,
    states: Vec<Option<State>>,
    active: Vec<bool>,
    templates: Vec<bool>,
}
impl Builder<'_> {
    fn variants(&self, id: TypeId) -> Vec<(String, Option<TypeId>)> {
        let ty = &self.image.types[id.index()];
        match &ty.constructor {
            T::Nominal(symbol) => self
                .image
                .definition(*symbol)
                .filter(|d| d.operation == TypeOperation::Enum)
                .and_then(|d| {
                    self.image.layout(id).map(|l| {
                        d.members
                            .iter()
                            .zip(&l.members)
                            .map(|(m, t)| (m.name.clone(), *t))
                            .collect()
                    })
                })
                .unwrap_or_default(),
            T::Enum(names) => {
                let mut args = ty.arguments.iter();
                names
                    .iter()
                    .map(|(name, payload)| {
                        (
                            name.clone(),
                            if *payload { args.next().copied() } else { None },
                        )
                    })
                    .collect()
            }
            T::Option | T::Result | T::FoldControl | T::PropertyTarget => (0..6)
                .filter_map(|i| {
                    let (name, _) = crate::type_image::builtin_variant(&ty.constructor, i)?;
                    let payload = crate::type_image::builtin_variant_argument(&ty.constructor, i)
                        .map(|a| ty.arguments[a]);
                    Some((name.into(), payload))
                })
                .collect(),
            _ => vec![],
        }
    }
    fn value(&mut self, id: TypeId) -> Result<State, String> {
        let i = id.index();
        if let Some(s) = &self.states[i] {
            return Ok(s.clone());
        }
        if self.templates[i] {
            self.states[i] = Some(State::Template);
            return Ok(State::Template);
        }
        if self.active[i] {
            return Ok(pending(
                "recursive inline payload requires an indirect representation rule",
                vec![i],
            ));
        }
        self.active[i] = true;
        let ty = &self.image.types[i];
        let args = ty.arguments.clone();
        let state = match &ty.constructor {
            T::Parameter(_) | T::Quantified(_) | T::TypeFunction(_) => State::Template,
            T::Int | T::Float | T::Bool => shape(8, 8, None)?,
            T::String => shape(16, 8, Some("StringTable"))?,
            // The outer stamp describes the value; data names the represented type.
            T::Type | T::TypeOf => shape(4, 4, None)?,
            T::Bytes => shape(12, 4, Some("BytesTable"))?,
            T::Array => shape(12, 4, Some("ArrayTable"))?,
            T::Dict => shape(4, 4, Some("DictTable"))?,
            T::Record(_) => shape(4, 4, Some("RecordTable"))?,
            T::Tuple if args.is_empty() => shape(0, 1, None)?,
            T::Unchecked => self.value(args[0])?,
            T::Nominal(symbol)
                if self
                    .image
                    .definition(*symbol)
                    .is_some_and(|d| d.operation == TypeOperation::Struct) =>
            {
                shape(4, 4, Some("RecordTable"))?
            }
            T::Nominal(symbol)
                if self
                    .image
                    .definition(*symbol)
                    .is_some_and(|d| d.operation == TypeOperation::Unit) =>
            {
                shape(0, 1, None)?
            }
            _ => {
                let variants = self.variants(id);
                let is_enum = matches!(ty.constructor, T::Enum(_))
                    || matches!(ty.constructor,
                    T::Nominal(s) if self.image.definition(s).is_some_and(|d| d.operation == TypeOperation::Enum));
                if variants.is_empty() && !is_enum {
                    pending(
                        "representation rule not yet specified",
                        args.iter().map(|a| a.index()).collect(),
                    )
                } else {
                    let mut max = 0;
                    let mut deps = vec![];
                    let mut template = false;
                    for (_, payload) in variants {
                        if let Some(p) = payload {
                            match self.value(p)? {
                                State::Known { shape } => max = max.max(shape.value_bytes),
                                State::Template => template = true,
                                State::Pending { .. } => deps.push(p.index()),
                            }
                        }
                    }
                    if template {
                        State::Template
                    } else if !deps.is_empty() {
                        pending("enum payload layout is pending", deps)
                    } else {
                        shape(
                            8u64.checked_add(max)
                                .ok_or("candidate layout size overflow")?,
                            8,
                            None,
                        )?
                    }
                }
            }
        };
        self.active[i] = false;
        self.states[i] = Some(state.clone());
        Ok(state)
    }
    fn object(&self, id: TypeId) -> Result<Option<Object>, String> {
        let State::Known { shape } = self.states[id.index()].as_ref().unwrap() else {
            return Ok(None);
        };
        let Some(table) = shape.table else {
            return Ok(None);
        };
        let mut ty = &self.image.types[id.index()];
        let mut owner = id;
        while ty.constructor == T::Unchecked {
            owner = ty.arguments[0];
            ty = &self.image.types[owner.index()];
        }
        let mut object = Object {
            status: "known",
            bytes: None,
            element_type: None,
            element_stride: None,
            members: vec![],
            reason: None,
        };
        match table {
            "StringTable" | "BytesTable" => {
                object.element_stride = Some(1);
            }
            "ArrayTable" => {
                let element = ty.arguments[0];
                object.element_type = Some(element.index());
                match self.states[element.index()].as_ref().unwrap() {
                    State::Known { shape } => object.element_stride = Some(shape.value_bytes),
                    State::Template => {
                        object.status = "template";
                    }
                    _ => {
                        object.status = "pending";
                        object.reason = Some("element layout is pending".into());
                    }
                }
            }
            "RecordTable" => {
                let fields: Vec<_> = match &ty.constructor {
                    T::Record(names) => names
                        .iter()
                        .cloned()
                        .zip(ty.arguments.iter().copied())
                        .collect(),
                    T::Nominal(s) => self
                        .image
                        .definition(*s)
                        .unwrap()
                        .members
                        .iter()
                        .zip(&self.image.layout(owner).unwrap().members)
                        .filter_map(|(m, t)| t.map(|t| (m.name.clone(), t)))
                        .collect(),
                    _ => unreachable!(),
                };
                let mut offset = Some(0u64);
                for (name, field) in fields {
                    object.members.push(Member {
                        name,
                        type_id: Some(field.index()),
                        offset,
                    });
                    match self.states[field.index()].as_ref().unwrap() {
                        State::Known { shape } => {
                            offset = offset
                                .map(|n| {
                                    n.checked_add(shape.value_bytes)
                                        .ok_or("candidate layout size overflow")
                                })
                                .transpose()?;
                        }
                        state => {
                            offset = None;
                            object.status = if matches!(state, State::Template) {
                                "template"
                            } else {
                                "pending"
                            };
                            object.reason = Some("field layout is not concrete".into());
                        }
                    }
                }
                object.bytes = offset;
            }
            "DictTable" => {
                object.status = "pending";
                object.reason = Some("hash storage layout is not yet specified".into());
            }
            _ => unreachable!(),
        }
        Ok(Some(object))
    }
}

/// Does not mutate the MIR, allocate runtime objects, or invoke inference.
pub fn calculate(sealed: &SealedMir<'_>) -> Result<Vec<Entry>, String> {
    let image = sealed.types();
    let mut templates: Vec<_> = image
        .types
        .iter()
        .map(|t| {
            matches!(
                t.constructor,
                T::Parameter(_) | T::Quantified(_) | T::TypeFunction(_)
            )
        })
        .collect();
    let mut users = vec![vec![]; image.types.len()];
    for (i, ty) in image.types.iter().enumerate() {
        for arg in &ty.arguments {
            users[arg.index()].push(i);
        }
    }
    let mut pending: Vec<_> = templates
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.then_some(i))
        .collect();
    while let Some(i) = pending.pop() {
        for &user in &users[i] {
            if !templates[user] {
                templates[user] = true;
                pending.push(user);
            }
        }
    }
    let mut b = Builder {
        image,
        states: vec![None; image.types.len()],
        active: vec![false; image.types.len()],
        templates,
    };
    for i in 0..image.types.len() {
        b.value(TypeId(i as u32))?;
    }
    (0..image.types.len())
        .map(|i| {
            let id = TypeId(i as u32);
            let variants = b
                .variants(id)
                .into_iter()
                .map(|(name, p)| Member {
                    name,
                    type_id: p.map(|p| p.index()),
                    offset: p
                        .filter(|_| matches!(b.states[i], Some(State::Known { .. })))
                        .map(|_| 24),
                })
                .collect();
            Ok(Entry {
                type_id: i,
                constructor: constructor_name(&image.types[i].constructor),
                layout: b.states[i].clone().unwrap(),
                object: b.object(id)?,
                variants,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checked_sizes() {
        for (data, alignment, expected) in [(0, 1, 16), (8, 8, 24), (12, 4, 32)] {
            let State::Known { shape } = shape(data, alignment, None).unwrap() else {
                unreachable!()
            };
            assert_eq!(shape.value_bytes, expected);
        }
        assert!(shape(u64::MAX, 8, None).is_err());
    }
}
