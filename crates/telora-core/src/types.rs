use crate::ast::{
    BinaryOperator, Binding, BindingKind, Block, Expr, ExprKind, Pattern, Program, StringPartKind,
    TypeArgumentKind, UnaryOperator, located,
};
use crate::compiler::compile_expression_with_external_bindings;
use crate::heap::{Handle, Heap, PersistentValue};
use crate::hir::{HirDefinitionId, HirDefinitionKind, HirExpressionId, HirProgram, HirResolution};
use crate::json::{Provenance, ValuePath, ValuePathSegment};
use crate::lexer::{FrontendError, SourceLocation};
use crate::lir::RegisterId;
use crate::parser::parse_registered;
use crate::semantic::{
    Conflict, DiagnosticId, FactIdentity, FactState, IncomputableReason, SemanticFact,
    UnknownReason,
};
use crate::source::{Diagnostic, SourceDatabase};
use crate::value::{
    Atom, Closure, CoreBuiltinTypeFunction, CoreDiagnosticFunction, CoreDynFunction,
    CoreModelFunction, NativeError, NativeFunction, Value,
};
use crate::{
    BuiltinAtom, CallContext, DebugSink, DiscardDebugSink, Quota, QuotaAccount, ValueKind,
    ValueRef, Vm,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

const DEFAULT_TOOL_FUEL: usize = 100_000;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TypeId(u32);

fn display_named_type(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

impl TypeId {
    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeNode {
    Pending,
    Ref(TypeId),
    Bound(TypeParameterId),
    Named(String),
    Declared {
        id: crate::value::DeclaredTypeId,
        name: String,
        body: TypeId,
    },
    Any,
    Never,
    Type,
    Dyn,
    TypeOf(TypeId),
    Int,
    Float,
    String,
    Bytes,
    Opaque(crate::NativeType),
    Atom(Atom),
    Array(TypeId),
    Dict(TypeId),
    Tagged {
        tag: Atom,
        payload: TypeId,
    },
    Tuple(Vec<TypeId>),
    Struct(BTreeMap<String, TypeId>),
    Enum(BTreeMap<String, Option<TypeId>>),
    Union(Vec<TypeId>),
    Function {
        parameters: Vec<TypeId>,
        result: TypeId,
    },
}

#[derive(Clone, Debug, Default)]
pub struct TypeGraph {
    nodes: Vec<TypeNode>,
    names: BTreeMap<String, TypeId>,
}

impl TypeGraph {
    pub fn node(&self, id: TypeId) -> &TypeNode {
        &self.nodes[id.index()]
    }

    pub fn named(&self, name: &str) -> Option<TypeId> {
        self.names.get(name).copied()
    }

    pub fn names(&self) -> impl Iterator<Item = (&str, TypeId)> {
        self.names.iter().map(|(name, id)| (name.as_str(), *id))
    }

    pub fn nodes(&self) -> impl ExactSizeIterator<Item = (TypeId, &TypeNode)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (TypeId(index as u32), node))
    }

    pub fn display(&self, id: TypeId) -> String {
        self.display_with(id, &mut HashSet::new())
    }

    pub fn is_assignable(&self, actual: TypeId, expected: TypeId) -> bool {
        self.assignable_with(actual, expected, &mut HashSet::new())
    }

    fn push(&mut self, node: TypeNode) -> TypeId {
        let id = TypeId(u32::try_from(self.nodes.len()).expect("type graph exceeds u32"));
        self.nodes.push(node);
        id
    }

    fn intern_descriptor(&mut self, descriptor: &TypeDescriptor) -> TypeId {
        let node = match descriptor {
            TypeDescriptor::Bound(parameter) => TypeNode::Bound(*parameter),
            TypeDescriptor::Named(name) => self
                .names
                .get(name)
                .copied()
                .map_or_else(|| TypeNode::Named(name.clone()), TypeNode::Ref),
            TypeDescriptor::Declared(declared) => TypeNode::Declared {
                id: declared.id.clone(),
                name: declared.name.clone(),
                body: self.intern_descriptor(&declared.body),
            },
            TypeDescriptor::Inference(_) => {
                unreachable!("solver descriptors must be explicitly erased before interning")
            }
            TypeDescriptor::Any => TypeNode::Any,
            TypeDescriptor::Never => TypeNode::Never,
            TypeDescriptor::Type => TypeNode::Type,
            TypeDescriptor::Dyn => TypeNode::Dyn,
            TypeDescriptor::TypeOf(instance) => TypeNode::TypeOf(self.intern_descriptor(instance)),
            TypeDescriptor::Int => TypeNode::Int,
            TypeDescriptor::Float => TypeNode::Float,
            TypeDescriptor::String => TypeNode::String,
            TypeDescriptor::Bytes => TypeNode::Bytes,
            TypeDescriptor::Opaque(native_type) => TypeNode::Opaque(native_type.clone()),
            TypeDescriptor::Atom(atom) => TypeNode::Atom(atom.clone()),
            TypeDescriptor::Array(item) => TypeNode::Array(self.intern_descriptor(item)),
            TypeDescriptor::Dict(item) => TypeNode::Dict(self.intern_descriptor(item)),
            TypeDescriptor::Tagged { tag, payload } => TypeNode::Tagged {
                tag: tag.clone(),
                payload: self.intern_descriptor(payload),
            },
            TypeDescriptor::Tuple(items) => TypeNode::Tuple(
                items
                    .iter()
                    .map(|item| self.intern_descriptor(item))
                    .collect(),
            ),
            TypeDescriptor::Struct(fields) => TypeNode::Struct(
                fields
                    .iter()
                    .map(|(name, item)| (name.clone(), self.intern_descriptor(item)))
                    .collect(),
            ),
            TypeDescriptor::Enum(variants) => TypeNode::Enum(
                variants
                    .iter()
                    .map(|(name, payload)| {
                        (
                            name.clone(),
                            payload.as_deref().map(|item| self.intern_descriptor(item)),
                        )
                    })
                    .collect(),
            ),
            TypeDescriptor::Union(variants) => TypeNode::Union(
                variants
                    .iter()
                    .map(|item| self.intern_descriptor(item))
                    .collect(),
            ),
            TypeDescriptor::Function { parameters, result } => TypeNode::Function {
                parameters: parameters
                    .iter()
                    .map(|item| self.intern_descriptor(item))
                    .collect(),
                result: self.intern_descriptor(result),
            },
        };
        self.push(node)
    }

    fn intern_erased_descriptor(&mut self, descriptor: &TypeDescriptor) -> TypeId {
        self.intern_descriptor(&erase_type_variables(descriptor))
    }

    fn install_named_descriptors(
        &mut self,
        descriptors: &BTreeMap<String, TypeDescriptor>,
    ) -> BTreeMap<String, TypeId> {
        let roots = descriptors
            .keys()
            .map(|name| {
                let id = self.push(TypeNode::Pending);
                self.names.insert(name.clone(), id);
                (name.clone(), id)
            })
            .collect::<BTreeMap<_, _>>();
        for (name, descriptor) in descriptors {
            let body = self.intern_descriptor(descriptor);
            self.nodes[roots[name].index()] = self.nodes[body.index()].clone();
        }
        roots
    }

    fn decode_persistent(
        &mut self,
        value: ValueRef<'_>,
        path: &str,
        links: &mut HashMap<Handle, TypeId>,
    ) -> Result<TypeId, String> {
        if let Some(handle) = value.hidden_up_link_handle() {
            if let Some(id) = links.get(&handle) {
                return Ok(*id);
            }
            let resolved = value.resolve_hidden_up_link().map_err(|message| {
                format!("{path} contains an uninitialized recursive type link: {message}")
            })?;
            let id = self.decode_persistent(resolved, path, links)?;
            links.insert(handle, id);
            return Ok(id);
        }
        if let Some(handle) = value.object_handle() {
            if let Some(id) = links.get(&handle) {
                return Ok(*id);
            }
            let id = self.push(TypeNode::Pending);
            links.insert(handle, id);
            let node = self.decode_persistent_node(value, path, links)?;
            self.nodes[id.index()] = node;
            return Ok(id);
        }
        let node = self.decode_persistent_node(value, path, links)?;
        Ok(self.push(node))
    }

    fn decode_persistent_node(
        &mut self,
        mut value: ValueRef<'_>,
        path: &str,
        links: &mut HashMap<Handle, TypeId>,
    ) -> Result<TypeNode, String> {
        if let Some(native_type) = value.as_native_type() {
            return Ok(TypeNode::Opaque(native_type.clone()));
        }
        if let Some((id, name, body)) = value.declared_type_parts() {
            return Ok(TypeNode::Declared {
                id: id.clone(),
                name: name.to_owned(),
                body: self.decode_persistent(body, path, links)?,
            });
        }
        loop {
            let fields = value
                .dict_fields()
                .ok_or_else(|| format!("{path} must be a Dict"))?;
            let kind = value
                .dict_get("kind")
                .and_then(ValueRef::as_atom)
                .ok_or_else(|| format!("{path}.kind must be an Atom"))?;
            if kind != "WithAttributes" {
                break;
            }
            if fields != ["attributes", "inner", "kind"] {
                return Err(format!("{path} has an invalid WithAttributes wrapper"));
            }
            let attributes = value.dict_get("attributes").expect("wrapper field exists");
            if attributes.kind() != ValueKind::Dict {
                return Err(format!("{path}.attributes must be a Dict"));
            }
            value = value.dict_get("inner").expect("wrapper field exists");
            if value.is_hidden_up_link()
                || value.as_native_type().is_some()
                || value.declared_type_parts().is_some()
            {
                let id = self.decode_persistent(value, path, links)?;
                return Ok(TypeNode::Ref(id));
            }
        }

        let fields = value.dict_fields().expect("metadata Dict checked above");
        let kind = value
            .dict_get("kind")
            .and_then(ValueRef::as_atom)
            .expect("metadata kind checked above");
        let require = |expected: &[&str]| {
            fields
                .iter()
                .copied()
                .eq(expected.iter().copied())
                .then_some(())
                .ok_or_else(|| format!("{path} has invalid fields for {kind}"))
        };
        Ok(match kind {
            "Bound" => {
                require(&["kind", "parameter"])?;
                let parameter = value
                    .dict_get("parameter")
                    .and_then(ValueRef::as_int)
                    .and_then(|parameter| u32::try_from(parameter).ok())
                    .ok_or_else(|| format!("{path}.parameter must be a non-negative Int"))?;
                TypeNode::Bound(TypeParameterId(parameter))
            }
            "Named" => {
                require(&["kind", "name"])?;
                let name = value
                    .dict_get("name")
                    .and_then(ValueRef::as_str)
                    .ok_or_else(|| format!("{path}.name must be a String"))?;
                TypeNode::Named(name.to_owned())
            }
            "Any" => {
                require(&["kind"])?;
                TypeNode::Any
            }
            "Never" => {
                require(&["kind"])?;
                TypeNode::Never
            }
            "Type" => {
                require(&["kind"])?;
                TypeNode::Type
            }
            "Dyn" => {
                require(&["kind"])?;
                TypeNode::Dyn
            }
            "TypeOf" => {
                require(&["instance", "kind"])?;
                let instance = self.decode_persistent(
                    value.dict_get("instance").expect("field exists"),
                    &format!("{path}.instance"),
                    links,
                )?;
                TypeNode::TypeOf(instance)
            }
            "Int" => {
                require(&["kind"])?;
                TypeNode::Int
            }
            "Float" => {
                require(&["kind"])?;
                TypeNode::Float
            }
            "String" => {
                require(&["kind"])?;
                TypeNode::String
            }
            "Bytes" => {
                require(&["kind"])?;
                TypeNode::Bytes
            }
            "Atom" => {
                require(&["kind", "tag"])?;
                let tag = value
                    .dict_get("tag")
                    .and_then(ValueRef::as_atom)
                    .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
                TypeNode::Atom(atom_from_name(tag))
            }
            "Array" => {
                require(&["item", "kind"])?;
                let item = self.decode_persistent(
                    value.dict_get("item").expect("field exists"),
                    &format!("{path}.item"),
                    links,
                )?;
                TypeNode::Array(item)
            }
            "Dict" => {
                require(&["item", "kind"])?;
                let item = self.decode_persistent(
                    value.dict_get("item").expect("field exists"),
                    &format!("{path}.item"),
                    links,
                )?;
                TypeNode::Dict(item)
            }
            "Tagged" => {
                require(&["kind", "payload", "tag"])?;
                let tag = value
                    .dict_get("tag")
                    .and_then(ValueRef::as_atom)
                    .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
                let payload = self.decode_persistent(
                    value.dict_get("payload").expect("field exists"),
                    &format!("{path}.payload"),
                    links,
                )?;
                TypeNode::Tagged {
                    tag: atom_from_name(tag),
                    payload,
                }
            }
            "Tuple" | "Union" => {
                let field = if kind == "Tuple" { "items" } else { "variants" };
                require(if kind == "Tuple" {
                    &["items", "kind"]
                } else {
                    &["kind", "variants"]
                })?;
                let sequence = value.dict_get(field).expect("field exists");
                if sequence.kind() != ValueKind::Array {
                    return Err(format!("{path}.{field} must be an Array"));
                }
                let mut values = Vec::new();
                for index in 0..sequence.sequence_len().expect("Array length") {
                    values.push(self.decode_persistent(
                        sequence.sequence_get(index).expect("Array item"),
                        &format!("{path}.{field}[{index}]"),
                        links,
                    )?);
                }
                if kind == "Union" && values.is_empty() {
                    return Err(format!("{path}.variants must not be empty"));
                }
                if kind == "Tuple" {
                    TypeNode::Tuple(values)
                } else {
                    TypeNode::Union(values)
                }
            }
            "Struct" => {
                require(&["fields", "kind"])?;
                let values = value.dict_get("fields").expect("field exists");
                let names = values
                    .dict_fields()
                    .ok_or_else(|| format!("{path}.fields must be a Dict"))?;
                let mut decoded = BTreeMap::new();
                for name in names {
                    let id = self.decode_persistent(
                        values.dict_get(name).expect("Dict field"),
                        &format!("{path}.fields.{name}"),
                        links,
                    )?;
                    decoded.insert(name.to_owned(), id);
                }
                TypeNode::Struct(decoded)
            }
            "Enum" => {
                require(&["kind", "variants"])?;
                let values = value.dict_get("variants").expect("field exists");
                let names = values
                    .dict_fields()
                    .ok_or_else(|| format!("{path}.variants must be a Dict"))?;
                if names.is_empty() {
                    return Err(format!("{path}.variants must not be empty"));
                }
                let mut decoded = BTreeMap::new();
                for name in names {
                    let variant_path = format!("{path}.variants.{name}");
                    let inner = strip_attributes_ref(
                        values.dict_get(name).expect("Dict field"),
                        &variant_path,
                    )?;
                    let payload = if inner.as_atom() == Some("None") {
                        None
                    } else {
                        Some(self.decode_persistent(inner, &variant_path, links)?)
                    };
                    decoded.insert(name.to_owned(), payload);
                }
                TypeNode::Enum(decoded)
            }
            "Func" => {
                require(&["kind", "parameters", "result"])?;
                let values = value.dict_get("parameters").expect("field exists");
                if values.kind() != ValueKind::Array {
                    return Err(format!("{path}.parameters must be an Array"));
                }
                let mut parameters = Vec::new();
                for index in 0..values.sequence_len().expect("Array length") {
                    parameters.push(self.decode_persistent(
                        values.sequence_get(index).expect("Array item"),
                        &format!("{path}.parameters[{index}]"),
                        links,
                    )?);
                }
                let result = self.decode_persistent(
                    value.dict_get("result").expect("field exists"),
                    &format!("{path}.result"),
                    links,
                )?;
                TypeNode::Function { parameters, result }
            }
            _ => return Err(format!("{path}.kind has unknown value '{kind}'")),
        })
    }

    fn display_with(&self, id: TypeId, active: &mut HashSet<TypeId>) -> String {
        if !active.insert(id) {
            return self
                .names
                .iter()
                .find_map(|(name, candidate)| {
                    (*candidate == id).then(|| display_named_type(name).to_owned())
                })
                .unwrap_or_else(|| "recursive".into());
        }
        let shown = match self.node(id) {
            TypeNode::Pending => "<pending>".into(),
            TypeNode::Ref(target) => self.display_with(*target, active),
            TypeNode::Bound(parameter) => format!("T{}", parameter.0),
            TypeNode::Named(name) => display_named_type(name).to_owned(),
            TypeNode::Declared { name, .. } => name.clone(),
            TypeNode::Any => "Any".into(),
            TypeNode::Never => "Never".into(),
            TypeNode::Type => "Type".into(),
            TypeNode::Dyn => "Dyn".into(),
            TypeNode::TypeOf(instance) => {
                format!("TypeOf({})", self.display_with(*instance, active))
            }
            TypeNode::Int => "Int".into(),
            TypeNode::Float => "Float".into(),
            TypeNode::String => "String".into(),
            TypeNode::Bytes => "Bytes".into(),
            TypeNode::Opaque(native_type) => {
                format!("opaque({})", native_type.qualified_name())
            }
            TypeNode::Atom(atom) => format!("'{}", atom.name()),
            TypeNode::Array(item) => format!("Array<{}>", self.display_with(*item, active)),
            TypeNode::Dict(item) => format!("Dict<{}>", self.display_with(*item, active)),
            TypeNode::Tagged { tag, payload } => {
                format!("'{}({})", tag.name(), self.display_with(*payload, active))
            }
            TypeNode::Tuple(items) => format!(
                "({})",
                items
                    .iter()
                    .map(|item| self.display_with(*item, active))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeNode::Struct(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(name, item)| format!("{name}: {}", self.display_with(*item, active)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeNode::Enum(variants) => format!(
                "enum {{{}}}",
                variants
                    .iter()
                    .map(|(name, payload)| payload.map_or_else(
                        || name.clone(),
                        |payload| format!("{name}({})", self.display_with(payload, active))
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TypeNode::Union(variants) => variants
                .iter()
                .map(|item| self.display_with(*item, active))
                .collect::<Vec<_>>()
                .join(" | "),
            TypeNode::Function { parameters, result } => format!(
                "Fn({}) -> {}",
                parameters
                    .iter()
                    .map(|item| self.display_with(*item, active))
                    .collect::<Vec<_>>()
                    .join(", "),
                self.display_with(*result, active)
            ),
        };
        active.remove(&id);
        shown
    }

    fn assignable_with(
        &self,
        actual: TypeId,
        expected: TypeId,
        visited: &mut HashSet<(TypeId, TypeId)>,
    ) -> bool {
        if !visited.insert((actual, expected)) {
            return true;
        }
        match (self.node(actual), self.node(expected)) {
            (TypeNode::Ref(actual), _) => self.assignable_with(*actual, expected, visited),
            (_, TypeNode::Ref(expected)) => self.assignable_with(actual, *expected, visited),
            (TypeNode::Bound(actual), TypeNode::Bound(expected)) => actual == expected,
            (TypeNode::Named(actual), TypeNode::Named(expected)) => actual == expected,
            (TypeNode::Declared { id: actual, .. }, TypeNode::Declared { id: expected, .. }) => {
                actual == expected
            }
            (TypeNode::Never, _) => true,
            (TypeNode::Any, _) | (_, TypeNode::Any) => true,
            (TypeNode::TypeOf(_), TypeNode::Type) => true,
            (TypeNode::TypeOf(a), TypeNode::TypeOf(e)) => self.assignable_with(*a, *e, visited),
            (TypeNode::Array(a), TypeNode::Array(e)) => self.assignable_with(*a, *e, visited),
            (TypeNode::Dict(a), TypeNode::Dict(e)) => self.assignable_with(*a, *e, visited),
            (TypeNode::Struct(fields), TypeNode::Dict(expected)) => fields
                .values()
                .all(|actual| self.assignable_with(*actual, *expected, visited)),
            (
                TypeNode::Tagged {
                    tag: a_tag,
                    payload: a,
                },
                TypeNode::Tagged {
                    tag: e_tag,
                    payload: e,
                },
            ) => a_tag == e_tag && self.assignable_with(*a, *e, visited),
            (TypeNode::Tuple(a), TypeNode::Tuple(e)) => {
                a.len() == e.len()
                    && a.iter()
                        .zip(e)
                        .all(|(a, e)| self.assignable_with(*a, *e, visited))
            }
            (TypeNode::Struct(a), TypeNode::Struct(e)) => {
                a.len() == e.len()
                    && e.iter().all(|(name, e)| {
                        a.get(name)
                            .is_some_and(|a| self.assignable_with(*a, *e, visited))
                    })
            }
            (TypeNode::Enum(a), TypeNode::Enum(e)) => {
                a.len() == e.len()
                    && e.iter().all(|(name, e)| {
                        a.get(name).is_some_and(|a| match (a, e) {
                            (None, None) => true,
                            (Some(a), Some(e)) => self.assignable_with(*a, *e, visited),
                            _ => false,
                        })
                    })
            }
            (TypeNode::Union(a), _) => a
                .iter()
                .all(|a| self.assignable_with(*a, expected, visited)),
            (_, TypeNode::Union(e)) => e.iter().any(|e| self.assignable_with(actual, *e, visited)),
            (
                TypeNode::Function {
                    parameters: ap,
                    result: ar,
                },
                TypeNode::Function {
                    parameters: ep,
                    result: er,
                },
            ) => {
                ap.len() == ep.len()
                    && ap
                        .iter()
                        .zip(ep)
                        .all(|(a, e)| self.assignable_with(*a, *e, visited))
                    && self.assignable_with(*ar, *er, visited)
            }
            (a, e) => a == e,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TypeParameterId(u32);

impl TypeParameterId {
    pub const fn index(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InferenceVariableId(u32);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeParameter {
    pub id: TypeParameterId,
    pub name: String,
    pub location: crate::Location,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeScheme {
    pub parameters: Vec<TypeParameter>,
    pub body: TypeDescriptor,
}

impl TypeScheme {
    pub fn display_name(&self) -> String {
        let names = self
            .parameters
            .iter()
            .map(|parameter| (parameter.id, parameter.name.as_str()))
            .collect::<HashMap<_, _>>();
        let body = display_scheme_descriptor(&self.body, &names);
        if self.parameters.is_empty() {
            body
        } else {
            format!(
                "for({}) {body}",
                self.parameters
                    .iter()
                    .map(|parameter| parameter.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ModuleInterface {
    pub exports: BTreeMap<String, TypeScheme>,
    pub concrete_types: BTreeMap<String, TypeDescriptor>,
    pub(crate) type_family_templates: BTreeMap<String, TypeFamilyTemplate>,
}

impl ModuleInterface {
    fn qualified(&self, namespace: &str) -> Self {
        let names = self
            .concrete_types
            .keys()
            .map(|name| (name.clone(), format!("\0import:{namespace}:{name}")))
            .collect::<HashMap<_, _>>();
        Self {
            exports: self
                .exports
                .iter()
                .map(|(name, scheme)| {
                    (
                        name.clone(),
                        TypeScheme {
                            parameters: scheme.parameters.clone(),
                            body: rename_named_types(&scheme.body, &names),
                        },
                    )
                })
                .collect(),
            concrete_types: self
                .concrete_types
                .iter()
                .map(|(name, descriptor)| {
                    (names[name].clone(), rename_named_types(descriptor, &names))
                })
                .collect(),
            type_family_templates: self
                .type_family_templates
                .iter()
                .map(|(name, family)| {
                    (
                        name.clone(),
                        TypeFamilyTemplate {
                            parameters: family.parameters.clone(),
                            metadata: rename_named_type_metadata(&family.metadata, &names),
                        },
                    )
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct TypeFamilyTemplate {
    parameters: Vec<TypeParameter>,
    metadata: Value,
}

fn rename_named_type_metadata(metadata: &Value, names: &HashMap<String, String>) -> Value {
    if let Value::DeclaredType(declared) = metadata {
        return Value::DeclaredType(crate::DeclaredType {
            id: declared.id().clone(),
            name: Arc::from(declared.name()),
            body: Box::new(rename_named_type_metadata(declared.body(), names)),
        });
    }
    let Value::Dict(fields) = metadata else {
        return metadata.clone();
    };
    let Some(Value::Atom(kind)) = fields.get("kind") else {
        return metadata.clone();
    };
    if kind.name() == "Named" {
        let Some(Value::String(name)) = fields.get("name") else {
            return metadata.clone();
        };
        let Some(renamed) = names.get(name.as_ref()) else {
            return metadata.clone();
        };
        let values = fields
            .shape()
            .fields()
            .iter()
            .zip(fields.values())
            .map(|(field, value)| {
                if field == "name" {
                    Value::string(renamed.as_str())
                } else {
                    value.clone()
                }
            })
            .collect();
        return Value::Dict(crate::Dict::new(fields.shape().clone(), values));
    }

    let values = fields
        .shape()
        .fields()
        .iter()
        .zip(fields.values())
        .map(|(field, value)| match (kind.name(), field.as_str()) {
            ("WithAttributes", "inner")
            | ("TypeOf", "instance")
            | ("Array" | "Dict", "item")
            | ("Tagged", "payload")
            | ("Func", "result") => rename_named_type_metadata(value, names),
            ("Tuple", "items") | ("Union", "variants") | ("Func", "parameters") => {
                rename_named_type_metadata_array(value, names)
            }
            ("Struct", "fields") => rename_named_type_metadata_dict(value, false, names),
            ("Enum", "variants") => rename_named_type_metadata_dict(value, true, names),
            _ => value.clone(),
        })
        .collect();
    Value::Dict(crate::Dict::new(fields.shape().clone(), values))
}

fn rename_named_type_metadata_array(metadata: &Value, names: &HashMap<String, String>) -> Value {
    let Value::Array(values) = metadata else {
        return metadata.clone();
    };
    Value::Array(
        values
            .iter()
            .map(|value| rename_named_type_metadata(value, names))
            .collect::<Vec<_>>()
            .into(),
    )
}

fn rename_named_type_metadata_dict(
    metadata: &Value,
    optional: bool,
    names: &HashMap<String, String>,
) -> Value {
    let Value::Dict(fields) = metadata else {
        return metadata.clone();
    };
    let values = fields
        .values()
        .iter()
        .map(|value| {
            if optional && optional_type_metadata_is_none(value) {
                value.clone()
            } else {
                rename_named_type_metadata(value, names)
            }
        })
        .collect();
    Value::Dict(crate::Dict::new(fields.shape().clone(), values))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeDescriptor {
    Bound(TypeParameterId),
    /// A static reference to a concrete type declaration. Runtime recursive
    /// metadata continues to use heap up-links; this variant preserves the
    /// declaration identity while contracts are analyzed.
    Named(String),
    Declared(DeclaredTypeDescriptor),
    Inference(InferenceVariableId),
    Any,
    Never,
    Type,
    Dyn,
    TypeOf(Box<TypeDescriptor>),
    Int,
    Float,
    String,
    Bytes,
    Opaque(crate::NativeType),
    Atom(Atom),
    Array(Box<TypeDescriptor>),
    Dict(Box<TypeDescriptor>),
    Tagged {
        tag: Atom,
        payload: Box<TypeDescriptor>,
    },
    Tuple(Vec<TypeDescriptor>),
    Struct(BTreeMap<String, TypeDescriptor>),
    Enum(BTreeMap<String, Option<Box<TypeDescriptor>>>),
    Union(Vec<TypeDescriptor>),
    Function {
        parameters: Vec<TypeDescriptor>,
        result: Box<TypeDescriptor>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclaredTypeDescriptor {
    pub(crate) id: crate::value::DeclaredTypeId,
    pub(crate) name: String,
    pub(crate) body: Arc<TypeDescriptor>,
}

impl DeclaredTypeDescriptor {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn body(&self) -> &TypeDescriptor {
        &self.body
    }
}

impl TypeDescriptor {
    pub(crate) fn identity_key(&self) -> String {
        fn sequence<'a>(tag: &str, items: impl IntoIterator<Item = &'a TypeDescriptor>) -> String {
            let items = items
                .into_iter()
                .map(TypeDescriptor::identity_key)
                .map(|item| format!("{}:{item}", item.len()))
                .collect::<String>();
            format!("{tag}{}:{items}", items.len())
        }

        match self {
            Self::Bound(parameter) => format!("b{}", parameter.0),
            Self::Named(name) => format!("n{}:{name}", name.len()),
            Self::Declared(declared) => format!("d{}", declared.id.identity_key()),
            Self::Inference(variable) => format!("i{}", variable.0),
            Self::Any => "a".into(),
            Self::Never => "v".into(),
            Self::Type => "t".into(),
            Self::Dyn => "y".into(),
            Self::TypeOf(instance) => sequence("o", [instance.as_ref()]),
            Self::Int => "I".into(),
            Self::Float => "F".into(),
            Self::String => "S".into(),
            Self::Bytes => "B".into(),
            Self::Opaque(native_type) => {
                let name = native_type.qualified_name();
                format!("p{}:{name}", name.len())
            }
            Self::Atom(atom) => {
                let name = atom.name();
                format!("m{}:{name}", name.len())
            }
            Self::Array(item) => sequence("r", [item.as_ref()]),
            Self::Dict(item) => sequence("c", [item.as_ref()]),
            Self::Tagged { tag, payload } => {
                let name = tag.name();
                format!("g{}:{name}{}", name.len(), sequence("", [payload.as_ref()]))
            }
            Self::Tuple(items) => sequence("u", items),
            Self::Struct(fields) => {
                let fields = fields
                    .iter()
                    .map(|(name, field)| {
                        let field = field.identity_key();
                        format!("{}:{name}{}:{field}", name.len(), field.len())
                    })
                    .collect::<String>();
                format!("s{}:{fields}", fields.len())
            }
            Self::Enum(variants) => {
                let variants = variants
                    .iter()
                    .map(|(name, payload)| {
                        let payload = payload
                            .as_deref()
                            .map(TypeDescriptor::identity_key)
                            .unwrap_or_default();
                        format!("{}:{name}{}:{payload}", name.len(), payload.len())
                    })
                    .collect::<String>();
                format!("e{}:{variants}", variants.len())
            }
            Self::Union(variants) => sequence("j", variants),
            Self::Function { parameters, result } => {
                format!(
                    "f{}{}",
                    sequence("", parameters),
                    sequence("", [result.as_ref()])
                )
            }
        }
    }

    pub fn to_value(&self, vm: &mut Vm) -> Value {
        let entries = match self {
            Self::Bound(parameter) => vec![
                kind_entry("Bound"),
                ("parameter".into(), Value::Int(i64::from(parameter.0))),
            ],
            Self::Named(name) => vec![
                kind_entry("Named"),
                ("name".into(), Value::string(name.as_str())),
            ],
            Self::Declared(declared) => {
                return Value::DeclaredType(crate::DeclaredType {
                    id: declared.id.clone(),
                    name: Arc::from(declared.name.as_str()),
                    body: Box::new(declared.body.to_value(vm)),
                });
            }
            Self::Inference(_) => panic!("inference variables are not runtime type metadata"),
            Self::Any => vec![kind_entry("Any")],
            Self::Never => vec![kind_entry("Never")],
            Self::Type => vec![kind_entry("Type")],
            Self::Dyn => vec![kind_entry("Dyn")],
            Self::TypeOf(instance) => {
                vec![
                    kind_entry("TypeOf"),
                    ("instance".into(), instance.to_value(vm)),
                ]
            }
            Self::Int => vec![kind_entry("Int")],
            Self::Float => vec![kind_entry("Float")],
            Self::String => vec![kind_entry("String")],
            Self::Bytes => vec![kind_entry("Bytes")],
            Self::Opaque(native_type) => return Value::NativeType(native_type.clone()),
            Self::Atom(tag) => vec![kind_entry("Atom"), ("tag".into(), Value::Atom(tag.clone()))],
            Self::Array(item) => vec![kind_entry("Array"), ("item".into(), item.to_value(vm))],
            Self::Dict(item) => vec![kind_entry("Dict"), ("item".into(), item.to_value(vm))],
            Self::Tagged { tag, payload } => vec![
                kind_entry("Tagged"),
                ("tag".into(), Value::Atom(tag.clone())),
                ("payload".into(), payload.to_value(vm)),
            ],
            Self::Tuple(items) => vec![
                kind_entry("Tuple"),
                (
                    "items".into(),
                    Value::Array(
                        items
                            .iter()
                            .map(|item| item.to_value(vm))
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                ),
            ],
            Self::Struct(fields) => {
                let field_values = fields
                    .iter()
                    .map(|(name, field)| (name.clone(), field.to_value(vm)))
                    .collect::<Vec<_>>();
                let fields = vm
                    .make_dict(field_values)
                    .expect("Type Struct fields are unique");
                vec![kind_entry("Struct"), ("fields".into(), fields)]
            }
            Self::Enum(variants) => {
                let variants = variants
                    .iter()
                    .map(|(name, payload)| {
                        (
                            name.clone(),
                            payload
                                .as_ref()
                                .map_or_else(Value::none, |payload| payload.to_value(vm)),
                        )
                    })
                    .collect::<Vec<_>>();
                let variants = vm
                    .make_dict(variants)
                    .expect("Type Enum variants are unique");
                vec![kind_entry("Enum"), ("variants".into(), variants)]
            }
            Self::Union(variants) => vec![
                kind_entry("Union"),
                (
                    "variants".into(),
                    Value::Array(
                        variants
                            .iter()
                            .map(|variant| variant.to_value(vm))
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                ),
            ],
            Self::Function { parameters, result } => vec![
                kind_entry("Func"),
                (
                    "parameters".into(),
                    Value::Array(
                        parameters
                            .iter()
                            .map(|parameter| parameter.to_value(vm))
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                ),
                ("result".into(), result.to_value(vm)),
            ],
        };
        vm.make_dict(entries)
            .expect("Type metadata fields are unique")
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        decode_type(value, "Type")
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::Bound(parameter) => format!("T{}", parameter.0),
            Self::Named(name) => display_named_type(name).to_owned(),
            Self::Declared(declared) => declared.name.clone(),
            Self::Inference(variable) => format!("?{}", variable.0),
            Self::Any => "Any".into(),
            Self::Never => "Never".into(),
            Self::Type => "Type".into(),
            Self::Dyn => "Dyn".into(),
            Self::TypeOf(instance) => format!("TypeOf({})", instance.display_name()),
            Self::Int => "Int".into(),
            Self::Float => "Float".into(),
            Self::String => "String".into(),
            Self::Bytes => "Bytes".into(),
            Self::Opaque(native_type) => format!("opaque({})", native_type.qualified_name()),
            Self::Atom(atom) => format!("'{}", atom.name()),
            Self::Array(item) => format!("Array<{}>", item.display_name()),
            Self::Dict(item) => format!("Dict<{}>", item.display_name()),
            Self::Tagged { tag, payload } => {
                format!("'{}({})", tag.name(), payload.display_name())
            }
            Self::Tuple(items) => format!(
                "({})",
                items
                    .iter()
                    .map(Self::display_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Struct(fields) => format!(
                "{{{}}}",
                fields
                    .iter()
                    .map(|(name, item)| format!("{name}: {}", item.display_name()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Enum(variants) => format!(
                "enum {{{}}}",
                variants
                    .iter()
                    .map(|(name, payload)| payload.as_ref().map_or_else(
                        || name.clone(),
                        |payload| format!("{name}({})", payload.display_name())
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Union(variants) => variants
                .iter()
                .map(Self::display_name)
                .collect::<Vec<_>>()
                .join(" | "),
            Self::Function { parameters, result } => format!(
                "Fn({}) -> {}",
                parameters
                    .iter()
                    .map(Self::display_name)
                    .collect::<Vec<_>>()
                    .join(", "),
                result.display_name()
            ),
        }
    }
}

fn display_scheme_descriptor(
    descriptor: &TypeDescriptor,
    names: &HashMap<TypeParameterId, &str>,
) -> String {
    match descriptor {
        TypeDescriptor::Bound(parameter) => names
            .get(parameter)
            .map_or_else(|| format!("T{}", parameter.0), |name| (*name).to_owned()),
        TypeDescriptor::Named(name) => display_named_type(name).to_owned(),
        TypeDescriptor::Declared(declared) => declared.name.clone(),
        TypeDescriptor::Inference(variable) => format!("?{}", variable.0),
        TypeDescriptor::Any => "Any".into(),
        TypeDescriptor::Never => "Never".into(),
        TypeDescriptor::Type => "Type".into(),
        TypeDescriptor::Dyn => "Dyn".into(),
        TypeDescriptor::TypeOf(instance) => {
            format!("TypeOf({})", display_scheme_descriptor(instance, names))
        }
        TypeDescriptor::Int => "Int".into(),
        TypeDescriptor::Float => "Float".into(),
        TypeDescriptor::String => "String".into(),
        TypeDescriptor::Bytes => "Bytes".into(),
        TypeDescriptor::Opaque(native_type) => {
            format!("opaque({})", native_type.qualified_name())
        }
        TypeDescriptor::Atom(atom) => format!("'{}", atom.name()),
        TypeDescriptor::Array(item) => {
            format!("Array<{}>", display_scheme_descriptor(item, names))
        }
        TypeDescriptor::Dict(item) => {
            format!("Dict<{}>", display_scheme_descriptor(item, names))
        }
        TypeDescriptor::Tagged { tag, payload } => format!(
            "'{}({})",
            tag.name(),
            display_scheme_descriptor(payload, names)
        ),
        TypeDescriptor::Tuple(items) => format!(
            "({})",
            items
                .iter()
                .map(|item| display_scheme_descriptor(item, names))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeDescriptor::Struct(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(name, item)| {
                    format!("{name}: {}", display_scheme_descriptor(item, names))
                })
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeDescriptor::Enum(variants) => format!(
            "enum {{{}}}",
            variants
                .iter()
                .map(|(name, payload)| payload.as_ref().map_or_else(
                    || name.clone(),
                    |payload| format!("{name}({})", display_scheme_descriptor(payload, names))
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeDescriptor::Union(variants) => variants
            .iter()
            .map(|variant| display_scheme_descriptor(variant, names))
            .collect::<Vec<_>>()
            .join(" | "),
        TypeDescriptor::Function { parameters, result } => format!(
            "Fn({}) -> {}",
            parameters
                .iter()
                .map(|parameter| display_scheme_descriptor(parameter, names))
                .collect::<Vec<_>>()
                .join(", "),
            display_scheme_descriptor(result, names)
        ),
    }
}

#[derive(Clone, Debug)]
pub struct Analysis {
    pub types: TypeGraph,
    pub declared_types: BTreeMap<String, TypeId>,
    pub binding_types: BTreeMap<String, TypeId>,
    pub result_type: TypeId,
    pub hir: HirProgram,
    pub definition_types: BTreeMap<HirDefinitionId, TypeId>,
    pub definition_schemes: BTreeMap<HirDefinitionId, TypeScheme>,
    pub expression_types: BTreeMap<HirExpressionId, TypeId>,
    pub module_interface: ModuleInterface,
    pub explicit_exports: bool,
    pub(crate) propagation_families: HashMap<crate::Location, PropagationFamily>,
    pub(crate) not_families: HashMap<crate::Location, NotFamily>,
    pub(crate) prelude: BTreeMap<String, Value>,
    pub(crate) external_values: BTreeMap<String, Value>,
    pub(crate) dynamic_bindings: HashSet<String>,
    pub(crate) type_family_values: BTreeMap<String, Value>,
    pub(crate) declared_value_owners: HashMap<crate::Location, Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PropagationFamily {
    Option,
    Result,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NotFamily {
    Bool,
    Int,
    Dynamic,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticDependencyNode {
    pub definition: HirDefinitionId,
    pub dependencies: Vec<HirDefinitionId>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SemanticDependencyGraph {
    pub nodes: Vec<SemanticDependencyNode>,
}

#[derive(Clone, Debug)]
pub struct PartialAnalysis {
    pub hir: HirProgram,
    pub dependencies: SemanticDependencyGraph,
    pub definition_facts: BTreeMap<HirDefinitionId, SemanticFact<TypeId>>,
    pub definition_schemes: BTreeMap<HirDefinitionId, TypeScheme>,
    pub diagnostics: Vec<Diagnostic>,
    pub types: TypeGraph,
}

impl Analysis {
    pub fn display(&self, id: TypeId) -> String {
        self.types.display(id)
    }

    pub(crate) fn install_promoted_types(
        &mut self,
        heap: &Heap,
        roots: &BTreeMap<String, PersistentValue>,
    ) -> Result<(), String> {
        let mut links = HashMap::<Handle, TypeId>::new();
        for (name, root) in roots {
            let id = self.types.decode_persistent(
                ValueRef::persistent(*root, heap),
                &format!("type {name}"),
                &mut links,
            )?;
            let witness = self.types.push(TypeNode::TypeOf(id));
            self.types.names.insert(name.clone(), id);
            self.declared_types.insert(name.clone(), id);
            self.binding_types.insert(name.clone(), witness);
            for definition in self.hir.definitions() {
                if definition.top_level
                    && definition.kind == HirDefinitionKind::Type
                    && definition.name == *name
                {
                    self.definition_types.insert(definition.id, witness);
                }
            }
        }
        Ok(())
    }
}

pub fn analyze_source(source_name: &str, source: &str) -> Result<Analysis, FrontendError> {
    analyze_source_with_fuel(source_name, source, DEFAULT_TOOL_FUEL)
}

pub fn analyze_source_with_fuel(
    source_name: &str,
    source: &str,
    evaluation_fuel: usize,
) -> Result<Analysis, FrontendError> {
    analyze_source_with_quota(source_name, source, Quota::with_fuel(evaluation_fuel))
}

pub fn analyze_source_with_quota(
    source_name: &str,
    source: &str,
    quota: Quota,
) -> Result<Analysis, FrontendError> {
    let mut sources = SourceDatabase::default();
    let source_id = sources.add(source_name, source);
    let parsed = parse_registered(&sources, source_id);
    let program = parsed.program.ok_or_else(|| {
        FrontendError::from_diagnostic(
            &sources,
            parsed
                .diagnostics
                .into_iter()
                .next()
                .expect("failed parse has a diagnostic"),
        )
    })?;
    let mut account = QuotaAccount::new(quota);
    analyze_program_with_bindings(
        source_name,
        &program,
        &mut account,
        &BTreeMap::new(),
        &HashSet::new(),
        &sources,
        &BTreeMap::new(),
    )
}

pub fn analyze_partial_types(source_name: &str, source: &str, quota: Quota) -> PartialAnalysis {
    analyze_partial_types_with_bindings(source_name, source, quota, &BTreeMap::new())
}

pub fn analyze_partial_types_with_bindings(
    source_name: &str,
    source: &str,
    quota: Quota,
    external_values: &BTreeMap<String, Value>,
) -> PartialAnalysis {
    let mut sources = SourceDatabase::default();
    let source_id = sources.add(source_name, source);
    analyze_partial_types_registered(&sources, source_id, quota, external_values, &HashSet::new())
}

pub(crate) fn analyze_partial_types_registered(
    sources: &SourceDatabase,
    source_id: crate::SourceId,
    quota: Quota,
    external_values: &BTreeMap<String, Value>,
    unavailable_imports: &HashSet<String>,
) -> PartialAnalysis {
    let parsed = parse_registered(sources, source_id);
    analyze_partial_types_recovered(
        sources,
        source_id,
        &parsed.recovered,
        parsed.diagnostics,
        quota,
        external_values,
        unavailable_imports,
    )
}

pub(crate) fn analyze_partial_types_recovered(
    sources: &SourceDatabase,
    source_id: crate::SourceId,
    recovered: &crate::parser::RecoveredProgram,
    initial_diagnostics: Vec<Diagnostic>,
    quota: Quota,
    external_values: &BTreeMap<String, Value>,
    unavailable_imports: &HashSet<String>,
) -> PartialAnalysis {
    analyze_partial_types_recovered_with_query(
        sources,
        source_id,
        recovered,
        initial_diagnostics,
        quota,
        external_values,
        PartialAnalysisControl {
            unavailable_imports,
            query: None,
        },
    )
}

pub(crate) struct PartialAnalysisControl<'a> {
    pub unavailable_imports: &'a HashSet<String>,
    pub query: Option<&'a crate::query::QueryContext>,
}

pub(crate) fn analyze_partial_types_recovered_with_query(
    sources: &SourceDatabase,
    source_id: crate::SourceId,
    recovered: &crate::parser::RecoveredProgram,
    initial_diagnostics: Vec<Diagnostic>,
    quota: Quota,
    external_values: &BTreeMap<String, Value>,
    control: PartialAnalysisControl<'_>,
) -> PartialAnalysis {
    let source_name = sources.get(source_id).name.to_string();
    let mut vm = Vm::new();
    let prelude = BootstrapPrelude::new(&mut vm);
    let hir = HirProgram::resolve_recovered(
        recovered,
        prelude
            .values
            .keys()
            .filter(|name| source_name.ends_with(".native.telora") || name.as_str() != "BlameError")
            .filter(|name| !external_values.contains_key(*name))
            .chain(external_values.keys())
            .cloned()
            .collect::<Vec<_>>(),
    );
    let bindings = type_definition_bindings(&hir, &recovered.bindings);
    let declared_initializer_slots = recovered
        .bindings
        .iter()
        .filter(|binding| binding.value.declared_initializer.is_some())
        .enumerate()
        .map(|(slot, binding)| (binding.value.name.location, slot as u32))
        .collect::<HashMap<_, _>>();
    let type_definitions = bindings.keys().copied().collect::<HashSet<_>>();
    let import_definitions = hir
        .definitions()
        .iter()
        .filter(|definition| {
            definition.top_level
                && definition.kind == HirDefinitionKind::Import
                && control.unavailable_imports.contains(&definition.name)
        })
        .map(|definition| definition.id)
        .collect::<HashSet<_>>();
    let mut unavailable_dependencies = BTreeMap::new();
    for definition in bindings.keys() {
        if let Some(import) = definition_dependencies(&hir, *definition)
            .into_iter()
            .find(|dependency| import_definitions.contains(dependency))
        {
            unavailable_dependencies.insert(*definition, import);
        }
    }
    let dependencies = type_dependency_graph(&hir, &type_definitions);

    let mut diagnostics = initial_diagnostics;
    let mut facts: BTreeMap<HirDefinitionId, SemanticFact<TypeId>> = BTreeMap::new();
    let mut definition_schemes = BTreeMap::new();
    for (definition, import) in unavailable_dependencies {
        let cause = FactIdentity::HirDefinition(import);
        let mut fact = SemanticFact::unknown(UnknownReason::BlockedBy(cause));
        fact.causes.push(cause);
        facts.insert(definition, fact);
    }
    let mut types = TypeGraph::default();
    let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
    let mut evaluator = ToolEvaluator::new(Arc::clone(&debug_sink));
    let mut tool_values = evaluator
        .publish_map(&prelude.values)
        .expect("core prelude values can enter the tool Main world");
    tool_values.extend(external_values.iter().map(|(name, value)| {
        (
            name.clone(),
            evaluator
                .publish(value)
                .expect("external values can enter the tool Main world"),
        )
    }));
    let any_metadata = *tool_values.get("Any").expect("core prelude defines Any");
    for binding in bindings.values() {
        tool_values.insert(binding.value.name.value.clone(), any_metadata);
    }
    for node in &dependencies.nodes {
        let binding = bindings[&node.definition];
        if binding.value.type_parameters.is_empty()
            && !binding.value.decorators.is_empty()
            && dependency_reaches(&dependencies, node.definition, node.definition)
        {
            let name = binding.value.name.value.clone();
            let value = TypeDescriptor::Named(name.clone()).to_value(&mut vm);
            tool_values.insert(
                name,
                evaluator
                    .publish(&value)
                    .expect("named metadata can enter the tool Main world"),
            );
        }
    }
    let mut account = QuotaAccount::new(quota);
    if let Some(query) = control.query {
        account = account.with_query(query.clone());
    }
    while facts.len() < bindings.len() {
        let mut progressed = false;
        for node in &dependencies.nodes {
            if facts.contains_key(&node.definition) {
                continue;
            }
            let blocked = node.dependencies.iter().find(|dependency| {
                facts
                    .get(*dependency)
                    .is_some_and(|fact| fact.state != FactState::Known)
            });
            if let Some(dependency) = blocked {
                let cause = FactIdentity::HirDefinition(*dependency);
                let mut fact = SemanticFact::unknown(UnknownReason::BlockedBy(cause));
                fact.causes.push(cause);
                facts.insert(node.definition, fact);
                progressed = true;
                continue;
            }
            if node
                .dependencies
                .iter()
                .any(|dependency| !facts.contains_key(dependency))
            {
                continue;
            }

            let binding = bindings[&node.definition];
            let mut evaluation_bindings = tool_values.clone();
            let mut parameters = Vec::new();
            let mut parameter_names = HashSet::new();
            for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
                if !parameter_names.insert(parameter.value.as_str()) {
                    let diagnostic = DiagnosticId::from_index(diagnostics.len());
                    diagnostics.push(Diagnostic::error(
                        format!("duplicate type parameter {:?}", parameter.value),
                        parameter.location,
                    ));
                    let mut fact = SemanticFact::conflicted(None, Conflict::IncompatibleContract);
                    fact.diagnostics.push(diagnostic);
                    facts.insert(node.definition, fact);
                    break;
                }
                let Ok(index) = u32::try_from(index) else {
                    let diagnostic = DiagnosticId::from_index(diagnostics.len());
                    diagnostics.push(Diagnostic::error(
                        "type family has too many parameters",
                        parameter.location,
                    ));
                    let mut fact =
                        SemanticFact::incomputable(None, IncomputableReason::UnsupportedOperation);
                    fact.diagnostics.push(diagnostic);
                    facts.insert(node.definition, fact);
                    break;
                };
                let id = TypeParameterId(index);
                parameters.push(TypeParameter {
                    id,
                    name: parameter.value.clone(),
                    location: parameter.location,
                });
                let value = TypeDescriptor::Bound(id).to_value(&mut vm);
                evaluation_bindings.insert(
                    parameter.value.clone(),
                    evaluator
                        .publish(&value)
                        .expect("bound metadata can enter the tool Main world"),
                );
            }
            if facts.contains_key(&node.definition) {
                progressed = true;
                continue;
            }
            let outcome = evaluate_tool_expression(
                &source_name,
                &binding.value.value,
                &evaluation_bindings,
                &mut account,
                sources,
                &mut evaluator,
            )
            .and_then(|value| {
                let value = if parameters.is_empty() {
                    declare_metadata_value(
                        &source_name,
                        binding,
                        &declared_initializer_slots,
                        value,
                        &mut evaluator,
                    )?
                } else if binding.value.declared_initializer.is_some() {
                    let arguments = parameters
                        .iter()
                        .map(|parameter| TypeDescriptor::Bound(parameter.id))
                        .collect::<Vec<_>>();
                    evaluator
                        .heap
                        .declare_persistent_type_application(
                            value,
                            source_name.as_str(),
                            declared_initializer_slots[&binding.value.name.location],
                            binding.value.name.value.as_str(),
                            &arguments,
                        )
                        .map_err(|error| frontend_error(&source_name, error.to_string()))?
                } else {
                    value
                };
                evaluator
                    .decode_type(value, "Type")
                    .map(|descriptor| (value, descriptor))
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!(
                                    "type {} produced invalid metadata: {message}",
                                    binding.value.name.value
                                ),
                                binding.value.value.location,
                            ),
                        )
                    })
            });
            match outcome {
                Ok((value, descriptor)) => {
                    let (definition_descriptor, published_value) = if parameters.is_empty() {
                        let declared = types.intern_descriptor(&descriptor);
                        types
                            .names
                            .insert(binding.value.name.value.clone(), declared);
                        (descriptor, value)
                    } else {
                        let mut bounds = Vec::new();
                        collect_bound_parameters(&descriptor, &mut bounds);
                        if let Some(foreign) = bounds.iter().find(|bound| {
                            !parameters.iter().any(|parameter| parameter.id == **bound)
                        }) {
                            let diagnostic = DiagnosticId::from_index(diagnostics.len());
                            diagnostics.push(Diagnostic::error(
                                format!(
                                    "type family {} produced foreign bound parameter T{}",
                                    binding.value.name.value, foreign.0
                                ),
                                binding.value.value.location,
                            ));
                            let mut fact =
                                SemanticFact::conflicted(None, Conflict::IncompatibleContract);
                            fact.diagnostics.push(diagnostic);
                            facts.insert(node.definition, fact);
                            progressed = true;
                            continue;
                        }
                        let metadata = evaluator
                            .export(value)
                            .expect("type-family metadata can cross the legacy analysis boundary");
                        let family = TypeFamilyTemplate {
                            parameters: parameters.clone(),
                            metadata,
                        };
                        let scheme = TypeScheme {
                            parameters,
                            body: TypeDescriptor::Function {
                                parameters: family
                                    .parameters
                                    .iter()
                                    .map(|parameter| {
                                        TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(
                                            parameter.id,
                                        )))
                                    })
                                    .collect(),
                                result: Box::new(TypeDescriptor::TypeOf(Box::new(descriptor))),
                            },
                        };
                        let erased = erase_type_variables(&scheme.body);
                        definition_schemes.insert(node.definition, scheme);
                        let family_value = type_family_value(&family);
                        let published = evaluator
                            .publish(&family_value)
                            .expect("type-family closure can enter the tool Main world");
                        (erased, published)
                    };
                    let id = types.intern_descriptor(&definition_descriptor);
                    tool_values.insert(binding.value.name.value.clone(), published_value);
                    facts.insert(node.definition, SemanticFact::known(id));
                }
                Err(error) => {
                    let state = classify_partial_error(&error.message);
                    let diagnostic = DiagnosticId::from_index(diagnostics.len());
                    diagnostics.push(error.diagnostic.map_or_else(
                        || Diagnostic::error(error.message, binding.value.value.location),
                        |diagnostic| *diagnostic,
                    ));
                    let mut fact = match state {
                        FactState::Conflicted(conflict) => SemanticFact::conflicted(None, conflict),
                        FactState::Incomputable(reason) => SemanticFact::incomputable(None, reason),
                        FactState::Unknown(reason) => SemanticFact::unknown(reason),
                        FactState::Known => unreachable!("errors cannot produce known facts"),
                    };
                    fact.diagnostics.push(diagnostic);
                    facts.insert(node.definition, fact);
                }
            }
            progressed = true;
        }
        if progressed {
            continue;
        }

        let cyclic = dependencies
            .nodes
            .iter()
            .filter(|node| !facts.contains_key(&node.definition))
            .filter(|node| dependency_reaches(&dependencies, node.definition, node.definition))
            .map(|node| node.definition)
            .collect::<Vec<_>>();
        let had_cycle = !cyclic.is_empty();
        let mut handled = HashSet::new();
        for root in cyclic {
            if !handled.insert(root) {
                continue;
            }
            let component = dependencies
                .nodes
                .iter()
                .map(|node| node.definition)
                .filter(|definition| !facts.contains_key(definition))
                .filter(|definition| {
                    dependency_reaches(&dependencies, root, *definition)
                        && dependency_reaches(&dependencies, *definition, root)
                })
                .collect::<Vec<_>>();
            handled.extend(component.iter().copied());
            let concrete_decorated = component.iter().all(|definition| {
                let binding = bindings[definition];
                binding.value.type_parameters.is_empty() && !binding.value.decorators.is_empty()
            });
            if concrete_decorated {
                let mut descriptors = BTreeMap::new();
                let mut values = BTreeMap::new();
                let mut failed = false;
                for definition in &component {
                    let binding = bindings[definition];
                    let outcome = evaluate_tool_expression(
                        &source_name,
                        &binding.value.value,
                        &tool_values,
                        &mut account,
                        sources,
                        &mut evaluator,
                    )
                    .and_then(|value| {
                        evaluator
                            .decode_type(value, "Type")
                            .map(|descriptor| (value, descriptor))
                            .map_err(|message| {
                                FrontendError::from_diagnostic(
                                    sources,
                                    Diagnostic::error(
                                        format!(
                                            "type {} produced invalid metadata: {message}",
                                            binding.value.name.value
                                        ),
                                        binding.value.value.location,
                                    ),
                                )
                            })
                    });
                    match outcome {
                        Ok((value, descriptor)) => {
                            descriptors.insert(binding.value.name.value.clone(), descriptor);
                            values.insert(binding.value.name.value.clone(), value);
                        }
                        Err(error) => {
                            let diagnostic = DiagnosticId::from_index(diagnostics.len());
                            diagnostics.push(error.diagnostic.map_or_else(
                                || Diagnostic::error(error.message, binding.value.value.location),
                                |diagnostic| *diagnostic,
                            ));
                            let mut fact = SemanticFact::incomputable(
                                None,
                                IncomputableReason::UnsupportedOperation,
                            );
                            fact.diagnostics.push(diagnostic);
                            facts.insert(*definition, fact);
                            failed = true;
                        }
                    }
                }
                if !failed {
                    let roots = types.install_named_descriptors(&descriptors);
                    for definition in &component {
                        let binding = bindings[definition];
                        let name = &binding.value.name.value;
                        facts.insert(*definition, SemanticFact::known(roots[name]));
                        tool_values.insert(name.clone(), values[name]);
                    }
                } else {
                    for definition in &component {
                        facts.entry(*definition).or_insert_with(|| {
                            SemanticFact::unknown(UnknownReason::BlockedBy(
                                FactIdentity::HirDefinition(root),
                            ))
                        });
                    }
                }
                continue;
            }
            for definition in component {
                let binding = bindings[&definition];
                let diagnostic = DiagnosticId::from_index(diagnostics.len());
                diagnostics.push(Diagnostic::error(
                    format!(
                        "recursive type component containing {:?} cannot be partially evaluated",
                        binding.value.name.value
                    ),
                    binding.value.name.location,
                ));
                let mut fact =
                    SemanticFact::incomputable(None, IncomputableReason::CyclicEvaluation);
                fact.diagnostics.push(diagnostic);
                facts.insert(definition, fact);
            }
        }
        if had_cycle {
            continue;
        }
        break;
    }
    let mut indexed_diagnostics = diagnostics.into_iter().enumerate().collect::<Vec<_>>();
    indexed_diagnostics.sort_by_key(|(_, diagnostic)| {
        diagnostic
            .labels
            .first()
            .map_or(0, |label| label.location.start)
    });
    let mut remapped_diagnostics = vec![DiagnosticId::from_index(0); indexed_diagnostics.len()];
    for (new, (old, _)) in indexed_diagnostics.iter().enumerate() {
        remapped_diagnostics[*old] = DiagnosticId::from_index(new);
    }
    for fact in facts.values_mut() {
        for diagnostic in &mut fact.diagnostics {
            *diagnostic = remapped_diagnostics[diagnostic.index()];
        }
    }
    let diagnostics = indexed_diagnostics
        .into_iter()
        .map(|(_, diagnostic)| diagnostic)
        .collect();
    PartialAnalysis {
        hir,
        dependencies,
        definition_facts: facts,
        definition_schemes,
        diagnostics,
        types,
    }
}

fn expression_descends_from(
    hir: &HirProgram,
    mut expression: HirExpressionId,
    root: HirExpressionId,
) -> bool {
    loop {
        if expression == root {
            return true;
        }
        let Some(parent) = hir
            .expression(expression)
            .and_then(|expression| expression.parent)
        else {
            return false;
        };
        expression = parent;
    }
}

fn dependency_reaches(
    graph: &SemanticDependencyGraph,
    current: HirDefinitionId,
    target: HirDefinitionId,
) -> bool {
    fn visit(
        graph: &SemanticDependencyGraph,
        current: HirDefinitionId,
        target: HirDefinitionId,
        visited: &mut HashSet<HirDefinitionId>,
    ) -> bool {
        let Some(node) = graph.nodes.iter().find(|node| node.definition == current) else {
            return false;
        };
        node.dependencies.iter().any(|dependency| {
            *dependency == target
                || visited.insert(*dependency) && visit(graph, *dependency, target, visited)
        })
    }
    visit(graph, current, target, &mut HashSet::new())
}

fn expression_dependencies(hir: &HirProgram, root: HirExpressionId) -> Vec<HirDefinitionId> {
    let mut dependencies = hir
        .expressions()
        .iter()
        .filter(|expression| expression_descends_from(hir, expression.id, root))
        .filter_map(|expression| expression.reference)
        .filter_map(|reference| hir.reference(reference))
        .filter_map(|reference| match reference.resolution {
            HirResolution::Definition(dependency) => Some(dependency),
            _ => None,
        })
        .collect::<Vec<_>>();
    dependencies.sort_unstable();
    dependencies.dedup();
    dependencies
}

fn definition_dependencies(hir: &HirProgram, definition: HirDefinitionId) -> Vec<HirDefinitionId> {
    let root = hir
        .definition(definition)
        .and_then(|definition| definition.value)
        .expect("type definition has a value expression");
    expression_dependencies(hir, root)
}

fn type_dependency_graph(
    hir: &HirProgram,
    type_definitions: &HashSet<HirDefinitionId>,
) -> SemanticDependencyGraph {
    let mut nodes = type_definitions
        .iter()
        .copied()
        .map(|definition| SemanticDependencyNode {
            definition,
            dependencies: definition_dependencies(hir, definition)
                .into_iter()
                .filter(|dependency| type_definitions.contains(dependency))
                .collect(),
        })
        .collect::<Vec<_>>();
    nodes.sort_by_key(|node| node.definition);
    SemanticDependencyGraph { nodes }
}

fn type_definition_bindings<'a>(
    hir: &HirProgram,
    bindings: &'a [Binding],
) -> BTreeMap<HirDefinitionId, &'a Binding> {
    bindings
        .iter()
        .filter(|binding| binding.value.kind == BindingKind::Type)
        .filter_map(|binding| {
            hir.definitions()
                .iter()
                .find(|definition| {
                    definition.top_level
                        && definition.kind == HirDefinitionKind::Type
                        && definition.location == binding.value.name.location
                })
                .map(|definition| (definition.id, binding))
        })
        .collect()
}

fn classify_partial_error(message: &str) -> FactState {
    if message.contains("not assignable") || message.contains("incompatible") {
        FactState::Conflicted(Conflict::IncompatibleContract)
    } else if message.contains("fuel exhausted")
        || message.contains("quota")
        || message.contains("stack limit")
    {
        FactState::Incomputable(IncomputableReason::QuotaExceeded)
    } else if message.contains("native symbol") || message.contains("has not been resolved") {
        FactState::Incomputable(IncomputableReason::RuntimeOnly)
    } else {
        FactState::Incomputable(IncomputableReason::UnsupportedOperation)
    }
}

pub(crate) fn analyze_program_registered(
    source_name: &str,
    sources: &SourceDatabase,
    program: &Program,
    evaluation_fuel: usize,
) -> Result<Analysis, FrontendError> {
    let mut account = QuotaAccount::new(Quota::with_fuel(evaluation_fuel));
    analyze_program_with_bindings(
        source_name,
        program,
        &mut account,
        &BTreeMap::new(),
        &HashSet::new(),
        sources,
        &BTreeMap::new(),
    )
}

pub(crate) fn analyze_program_with_bindings(
    source_name: &str,
    program: &Program,
    account: &mut QuotaAccount,
    external_values: &BTreeMap<String, Value>,
    dynamic_bindings: &HashSet<String>,
    sources: &SourceDatabase,
    external_provenance: &BTreeMap<String, Provenance>,
) -> Result<Analysis, FrontendError> {
    let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
    analyze_program_with_bindings_observed(
        source_name,
        program,
        account,
        external_values,
        dynamic_bindings,
        sources,
        external_provenance,
        &BTreeMap::new(),
        &debug_sink,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn analyze_program_with_bindings_observed(
    source_name: &str,
    program: &Program,
    account: &mut QuotaAccount,
    external_values: &BTreeMap<String, Value>,
    dynamic_bindings: &HashSet<String>,
    sources: &SourceDatabase,
    external_provenance: &BTreeMap<String, Provenance>,
    external_interfaces: &BTreeMap<String, ModuleInterface>,
    debug_sink: &Arc<dyn DebugSink>,
) -> Result<Analysis, FrontendError> {
    let mut tool_vm = Vm::new();
    let prelude = BootstrapPrelude::new(&mut tool_vm);
    let hir = HirProgram::resolve(
        program,
        prelude
            .values
            .keys()
            .filter(|name| source_name.ends_with(".native.telora") || name.as_str() != "BlameError")
            .filter(|name| !external_values.contains_key(*name))
            .chain(external_values.keys())
            .cloned()
            .collect::<Vec<_>>(),
    );
    let prelude_values = prelude
        .values
        .iter()
        .filter(|(name, _)| !external_values.contains_key(*name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let native_abi = source_name.ends_with(".native.telora");
    let prelude_names = prelude
        .values
        .keys()
        .filter(|name| native_abi || name.as_str() != "BlameError")
        .cloned()
        .collect::<Vec<_>>();
    let BootstrapPrelude {
        values: bootstrap_values,
        types: mut static_environment,
        schemes: mut binding_schemes,
    } = prelude;
    let mut evaluator = ToolEvaluator::new(Arc::clone(debug_sink));
    let mut tool_values = evaluator.publish_map(&bootstrap_values)?;
    let mut declared_types = BTreeMap::new();
    let mut binding_types = BTreeMap::new();
    let mut declared_type_spans = HashMap::new();
    let mut expression_descriptors = HashMap::new();
    let declared_initializer_slots = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| binding.value.declared_initializer.is_some())
        .enumerate()
        .map(|(slot, binding)| (binding.value.name.location, slot as u32))
        .collect::<HashMap<_, _>>();
    let qualified_external_interfaces = external_interfaces
        .iter()
        .map(|(name, interface)| (name.clone(), interface.qualified(name)))
        .collect::<BTreeMap<_, _>>();

    let authored_names = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| {
            !matches!(
                binding.value.kind,
                BindingKind::OpenImport | BindingKind::Export
            )
        })
        .map(|binding| binding.value.name.value.as_str())
        .collect::<HashSet<_>>();
    for (name, value) in external_values {
        if authored_names.contains(name.as_str()) {
            continue;
        }
        let interface = qualified_external_interfaces.get(name);
        let scheme = interface
            .and_then(|interface| interface.exports.get(name))
            .cloned();
        let tool_value = authoritative_imported_value(value, interface, name, &mut tool_vm);
        tool_values.insert(name.clone(), evaluator.publish(&tool_value)?);
        let inferred = imported_static_descriptor(value, interface, name);
        static_environment.insert(name.clone(), inferred.clone());
        binding_types.insert(name.clone(), inferred);
        if let Some(scheme) = scheme {
            binding_schemes.insert(name.clone(), scheme);
        }
    }
    let imported_named_types = qualified_external_interfaces
        .values()
        .flat_map(|interface| interface.concrete_types.clone())
        .collect::<BTreeMap<_, _>>();
    validate_export_references(program, prelude_names.iter(), external_values, sources)?;

    let any_metadata = *tool_values.get("Any").expect("core prelude defines Any");
    for binding in &program.value.body.value.bindings {
        if matches!(
            binding.value.kind,
            BindingKind::Type | BindingKind::NativeType
        ) {
            tool_values.insert(binding.value.name.value.clone(), any_metadata);
            static_environment.insert(binding.value.name.value.clone(), TypeDescriptor::Type);
            binding_types.insert(binding.value.name.value.clone(), TypeDescriptor::Type);
        }
    }

    for name in dynamic_bindings {
        if !external_values.contains_key(name) {
            return Err(frontend_error(
                source_name,
                format!("dynamic binding {name:?} has no value"),
            ));
        }
        static_environment.insert(name.clone(), TypeDescriptor::Any);
        binding_types.insert(name.clone(), TypeDescriptor::Any);
    }

    // Definition contracts are evaluated before the source-order binding pass.
    // Make resolved imports available at that same tool stage so selectively
    // imported TypeMetadata can be used directly as a contract.
    for binding in &program.value.body.value.bindings {
        if binding.value.kind != BindingKind::Import {
            continue;
        }
        let name = &binding.value.name.value;
        let value = external_values.get(name).cloned().ok_or_else(|| {
            frontend_error(source_name, format!("import {name} has not been resolved"))
        })?;
        let interface = qualified_external_interfaces.get(name);
        let value = authoritative_imported_value(&value, interface, name, &mut tool_vm);
        tool_values.insert(name.clone(), evaluator.publish(&value)?);
    }

    let type_bindings = type_definition_bindings(&hir, &program.value.body.value.bindings);
    let type_definitions = type_bindings.keys().copied().collect::<HashSet<_>>();
    let type_dependencies = type_dependency_graph(&hir, &type_definitions);
    for node in &type_dependencies.nodes {
        let binding = type_bindings[&node.definition];
        if binding.value.type_parameters.is_empty()
            && !binding.value.decorators.is_empty()
            && dependency_reaches(&type_dependencies, node.definition, node.definition)
        {
            let name = binding.value.name.value.clone();
            let value = TypeDescriptor::Named(name.clone()).to_value(&mut tool_vm);
            tool_values.insert(name, evaluator.publish(&value)?);
        }
    }
    let contract_type_definitions = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| {
            matches!(binding.value.kind, BindingKind::Decl | BindingKind::Native)
                || binding.value.kind == BindingKind::Def && binding.value.annotation.is_some()
        })
        .filter_map(|binding| binding.value.annotation.as_ref())
        .flat_map(|annotation| hir.expression_ids_at(annotation.location))
        .flat_map(|root| expression_dependencies(&hir, root))
        .filter(|definition| type_definitions.contains(definition))
        .collect::<Vec<_>>();
    let family_definitions = type_bindings
        .iter()
        .filter(|(_, binding)| !binding.value.type_parameters.is_empty())
        .map(|(definition, _)| *definition)
        .collect::<Vec<_>>();
    let family_dependents = type_definitions
        .iter()
        .copied()
        .filter(|definition| {
            family_definitions.iter().any(|family| {
                *definition == *family
                    || dependency_reaches(&type_dependencies, *definition, *family)
            })
        })
        .collect::<Vec<_>>();
    let mut scheduled_types = BTreeSet::new();
    let mut frontier = family_dependents;
    frontier.extend(contract_type_definitions);
    while let Some(definition) = frontier.pop() {
        if !scheduled_types.insert(definition) {
            continue;
        }
        if let Some(node) = type_dependencies
            .nodes
            .iter()
            .find(|node| node.definition == definition)
        {
            frontier.extend(node.dependencies.iter().copied());
        }
    }

    let mut pending_types = scheduled_types.clone();
    let mut evaluated_types = BTreeSet::new();
    let mut evaluated_concrete_type_names = HashSet::new();
    let mut type_family_values = BTreeMap::new();
    let mut type_family_templates = BTreeMap::new();
    while !pending_types.is_empty() {
        let mut progressed = false;
        for definition in pending_types.iter().copied().collect::<Vec<_>>() {
            let node = type_dependencies
                .nodes
                .iter()
                .find(|node| node.definition == definition)
                .expect("scheduled type has a dependency node");
            if node
                .dependencies
                .iter()
                .any(|dependency| !evaluated_types.contains(dependency))
            {
                continue;
            }
            let binding = type_bindings[&definition];
            if binding.value.type_parameters.is_empty() {
                let value = evaluate_tool_expression(
                    source_name,
                    &binding.value.value,
                    &tool_values,
                    account,
                    sources,
                    &mut evaluator,
                )?;
                let value = declare_metadata_value(
                    source_name,
                    binding,
                    &declared_initializer_slots,
                    value,
                    &mut evaluator,
                )?;
                let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!(
                                "type {} produced invalid metadata: {message}",
                                binding.value.name.value
                            ),
                            binding.value.value.location,
                        ),
                    )
                })?;
                let name = binding.value.name.value.clone();
                declared_types.insert(name.clone(), descriptor.clone());
                declared_type_spans.insert(name.clone(), binding.location);
                tool_values.insert(name.clone(), value);
                let witness = TypeDescriptor::TypeOf(Box::new(descriptor));
                static_environment.insert(name.clone(), witness.clone());
                binding_types.insert(name.clone(), witness.clone());
                binding_schemes.insert(
                    name.clone(),
                    TypeScheme {
                        parameters: Vec::new(),
                        body: witness,
                    },
                );
                evaluated_concrete_type_names.insert(name);
                pending_types.remove(&definition);
                evaluated_types.insert(definition);
                progressed = true;
                continue;
            }

            let mut names = HashSet::new();
            let mut parameters = Vec::new();
            let mut bindings = tool_values.clone();
            for (parameter_index, parameter) in binding.value.type_parameters.iter().enumerate() {
                if !names.insert(parameter.value.as_str()) {
                    return Err(FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!("duplicate type parameter {:?}", parameter.value),
                            parameter.location,
                        ),
                    ));
                }
                let parameter_id =
                    TypeParameterId(u32::try_from(parameter_index).map_err(|_| {
                        frontend_error(source_name, "type family has too many parameters")
                    })?);
                parameters.push(TypeParameter {
                    id: parameter_id,
                    name: parameter.value.clone(),
                    location: parameter.location,
                });
                let value = TypeDescriptor::Bound(parameter_id).to_value(&mut tool_vm);
                bindings.insert(parameter.value.clone(), evaluator.publish(&value)?);
            }
            let value = evaluate_tool_expression(
                source_name,
                &binding.value.value,
                &bindings,
                account,
                sources,
                &mut evaluator,
            )?;
            let value = if binding.value.declared_initializer.is_some() {
                let arguments = parameters
                    .iter()
                    .map(|parameter| TypeDescriptor::Bound(parameter.id))
                    .collect::<Vec<_>>();
                evaluator
                    .heap
                    .declare_persistent_type_application(
                        value,
                        source_name,
                        declared_initializer_slots[&binding.value.name.location],
                        binding.value.name.value.as_str(),
                        &arguments,
                    )
                    .map_err(|error| frontend_error(source_name, error.to_string()))?
            } else {
                value
            };
            let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
                FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!(
                            "type family {} produced invalid metadata: {message}",
                            binding.value.name.value
                        ),
                        binding.value.value.location,
                    ),
                )
            })?;
            let mut bounds = Vec::new();
            collect_bound_parameters(&descriptor, &mut bounds);
            if let Some(foreign) = bounds
                .iter()
                .find(|bound| !parameters.iter().any(|parameter| parameter.id == **bound))
            {
                return Err(FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!(
                            "type family {} produced foreign bound parameter T{}",
                            binding.value.name.value, foreign.0
                        ),
                        binding.value.value.location,
                    ),
                ));
            }
            let family = TypeFamilyTemplate {
                parameters: parameters.clone(),
                metadata: evaluator.export(value)?,
            };
            let family_value = type_family_value(&family);
            let scheme = TypeScheme {
                parameters,
                body: TypeDescriptor::Function {
                    parameters: family
                        .parameters
                        .iter()
                        .map(|parameter| {
                            TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(parameter.id)))
                        })
                        .collect(),
                    result: Box::new(TypeDescriptor::TypeOf(Box::new(descriptor))),
                },
            };
            let erased = erase_type_variables(&scheme.body);
            tool_values.insert(
                binding.value.name.value.clone(),
                evaluator.publish(&family_value)?,
            );
            static_environment.insert(binding.value.name.value.clone(), erased.clone());
            binding_types.insert(binding.value.name.value.clone(), erased);
            binding_schemes.insert(binding.value.name.value.clone(), scheme);
            type_family_templates.insert(binding.value.name.value.clone(), family.clone());
            type_family_values.insert(binding.value.name.value.clone(), family_value);
            pending_types.remove(&definition);
            evaluated_types.insert(definition);
            progressed = true;
        }
        if !progressed {
            let root = pending_types
                .iter()
                .copied()
                .find(|definition| dependency_reaches(&type_dependencies, *definition, *definition))
                .expect("stalled type dependency schedule contains a cycle");
            let mut component = pending_types
                .iter()
                .copied()
                .filter(|definition| {
                    dependency_reaches(&type_dependencies, root, *definition)
                        && dependency_reaches(&type_dependencies, *definition, root)
                })
                .collect::<Vec<_>>();
            component.sort_unstable();
            let names = component
                .iter()
                .map(|definition| type_bindings[definition].value.name.value.as_str())
                .collect::<Vec<_>>();
            let binding = type_bindings[&root];
            let contains_family = component
                .iter()
                .any(|definition| !type_bindings[definition].value.type_parameters.is_empty());
            let concrete_decorated = !contains_family
                && component
                    .iter()
                    .all(|definition| !type_bindings[definition].value.decorators.is_empty());
            if concrete_decorated {
                for definition in component {
                    let binding = type_bindings[&definition];
                    let value = evaluate_tool_expression(
                        source_name,
                        &binding.value.value,
                        &tool_values,
                        account,
                        sources,
                        &mut evaluator,
                    )?;
                    let value = declare_metadata_value(
                        source_name,
                        binding,
                        &declared_initializer_slots,
                        value,
                        &mut evaluator,
                    )?;
                    let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
                        frontend_error(
                            source_name,
                            format!(
                                "type {} produced invalid metadata: {message}",
                                binding.value.name.value
                            ),
                        )
                    })?;
                    let name = binding.value.name.value.clone();
                    declared_types.insert(name.clone(), descriptor.clone());
                    declared_type_spans.insert(name.clone(), binding.location);
                    tool_values.insert(name.clone(), value);
                    let witness = TypeDescriptor::TypeOf(Box::new(descriptor));
                    static_environment.insert(name.clone(), witness.clone());
                    binding_types.insert(name.clone(), witness.clone());
                    binding_schemes.insert(
                        name.clone(),
                        TypeScheme {
                            parameters: Vec::new(),
                            body: witness,
                        },
                    );
                    evaluated_concrete_type_names.insert(name);
                    pending_types.remove(&definition);
                    evaluated_types.insert(definition);
                }
                continue;
            }
            let message = if contains_family {
                format!("recursive type family component containing {names:?} is not supported")
            } else {
                format!(
                    "recursive type component required by a definition contract containing {names:?} is not supported"
                )
            };
            let mut diagnostic = Diagnostic::error(message, binding.value.name.location);
            for definition in component {
                if definition == root {
                    continue;
                }
                let participant = type_bindings[&definition];
                diagnostic =
                    diagnostic.with_secondary("cycle participant", participant.value.name.location);
            }
            return Err(FrontendError::from_diagnostic(sources, diagnostic));
        }
    }

    let mut definition_contracts = HashMap::new();
    let mut declaration_locations = HashMap::new();
    let mut definition_counts = HashMap::<String, usize>::new();
    for binding in &program.value.body.value.bindings {
        let name = &binding.value.name.value;
        if binding.value.kind == BindingKind::Def {
            *definition_counts.entry(name.clone()).or_default() += 1;
        }
        if !matches!(binding.value.kind, BindingKind::Decl | BindingKind::Native)
            && !(binding.value.kind == BindingKind::Def && binding.value.annotation.is_some())
        {
            continue;
        }
        if definition_contracts.contains_key(name) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(format!("duplicate declaration {name:?}"), binding.location),
            ));
        }
        let contract = binding
            .value
            .annotation
            .as_ref()
            .expect("declaration has a lowered contract");
        let mut contract_values = tool_values.clone();
        let mut parameter_names = HashSet::new();
        let mut scheme_parameters = Vec::new();
        for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
            if !parameter_names.insert(parameter.value.clone()) {
                return Err(FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!("duplicate type parameter {:?}", parameter.value),
                        parameter.location,
                    ),
                ));
            }
            let id = TypeParameterId(index as u32);
            scheme_parameters.push(TypeParameter {
                id,
                name: parameter.value.clone(),
                location: parameter.location,
            });
            let value = TypeDescriptor::Bound(id).to_value(&mut tool_vm);
            contract_values.insert(parameter.value.clone(), evaluator.publish(&value)?);
        }
        let metadata = evaluate_tool_expression(
            source_name,
            contract,
            &contract_values,
            account,
            sources,
            &mut evaluator,
        )?;
        let descriptor = evaluator.decode_type(metadata, "Type").map_err(|message| {
            frontend_error(
                source_name,
                format!("declaration {name} has invalid contract metadata: {message}"),
            )
        })?;
        if binding.value.kind != BindingKind::Native
            || !scheme_parameters.is_empty()
            || contains_metatype(&descriptor)
        {
            binding_schemes.insert(
                name.clone(),
                TypeScheme {
                    parameters: scheme_parameters,
                    body: descriptor.clone(),
                },
            );
        }
        let erased = erase_type_variables(&descriptor);
        static_environment.insert(name.clone(), erased.clone());
        binding_types.insert(name.clone(), erased);
        if matches!(binding.value.kind, BindingKind::Decl | BindingKind::Def) {
            definition_contracts.insert(name.clone(), descriptor);
            declaration_locations.insert(name.clone(), binding.location);
        }
    }
    for (name, count) in &definition_counts {
        if *count > 1 {
            return Err(frontend_error(
                source_name,
                format!("definition {name:?} is initialized more than once"),
            ));
        }
    }

    for binding in &program.value.body.value.bindings {
        if matches!(binding.value.value.value, ExprKind::Interpreter { .. }) {
            let contract = definition_contracts.get(&binding.value.name.value);
            validate_interpreter_contract(&binding.value.type_parameters, contract).map_err(
                |message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(message, binding.value.value.location),
                    )
                },
            )?;
        }
    }
    if let Some(reference) = hir.unresolved().next() {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!("unknown binding {:?}", reference.name),
                reference.location,
            ),
        ));
    }

    for binding in &program.value.body.value.bindings {
        check_interpolations(&binding.value.value, &static_environment, sources)?;
        let inferred_expression = infer_expr_recorded(
            &binding.value.value,
            &static_environment,
            &mut expression_descriptors,
        );
        if let Some(annotation) = &binding.value.annotation {
            check_interpolations(annotation, &static_environment, sources)?;
            infer_expr_recorded(annotation, &static_environment, &mut expression_descriptors);
        }
        match binding.value.kind {
            BindingKind::OpenImport | BindingKind::Export => continue,
            BindingKind::Decl => continue,
            BindingKind::Native | BindingKind::NativeType => {
                let value = external_values
                    .get(&binding.value.name.value)
                    .cloned()
                    .ok_or_else(|| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!(
                                    "native symbol {:?} has not been linked",
                                    binding.value.name.value
                                ),
                                binding.location,
                            ),
                        )
                    })?;
                tool_values.insert(binding.value.name.value.clone(), evaluator.publish(&value)?);
                if binding.value.kind == BindingKind::NativeType {
                    let value = tool_values[&binding.value.name.value];
                    let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!(
                                    "native type {} is invalid: {message}",
                                    binding.value.name.value
                                ),
                                binding.location,
                            ),
                        )
                    })?;
                    let witness = TypeDescriptor::TypeOf(Box::new(descriptor.clone()));
                    declared_types.insert(binding.value.name.value.clone(), descriptor);
                    static_environment.insert(binding.value.name.value.clone(), witness.clone());
                    binding_types.insert(binding.value.name.value.clone(), witness.clone());
                    binding_schemes.insert(
                        binding.value.name.value.clone(),
                        TypeScheme {
                            parameters: Vec::new(),
                            body: witness,
                        },
                    );
                }
            }
            BindingKind::Type => {
                if !binding.value.type_parameters.is_empty()
                    || evaluated_concrete_type_names.contains(&binding.value.name.value)
                {
                    continue;
                }
                let value = evaluate_tool_expression(
                    source_name,
                    &binding.value.value,
                    &tool_values,
                    account,
                    sources,
                    &mut evaluator,
                )?;
                let value = declare_metadata_value(
                    source_name,
                    binding,
                    &declared_initializer_slots,
                    value,
                    &mut evaluator,
                )?;
                let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!(
                                "type {} produced invalid metadata: {message}",
                                binding.value.name.value
                            ),
                            binding.value.value.location,
                        ),
                    )
                })?;
                declared_types.insert(binding.value.name.value.clone(), descriptor);
                declared_type_spans.insert(binding.value.name.value.clone(), binding.location);
                tool_values.insert(binding.value.name.value.clone(), value);
                let witness = TypeDescriptor::TypeOf(Box::new(
                    declared_types[&binding.value.name.value].clone(),
                ));
                static_environment.insert(binding.value.name.value.clone(), witness.clone());
                binding_types.insert(binding.value.name.value.clone(), witness.clone());
                binding_schemes.insert(
                    binding.value.name.value.clone(),
                    TypeScheme {
                        parameters: Vec::new(),
                        body: witness,
                    },
                );
            }
            BindingKind::Let => {
                let inferred = inferred_expression;
                let checked = if let Some(annotation) = &binding.value.annotation {
                    let metadata = evaluate_tool_expression(
                        source_name,
                        annotation,
                        &tool_values,
                        account,
                        sources,
                        &mut evaluator,
                    )?;
                    let expected = evaluator.decode_type(metadata, "Type").map_err(|message| {
                        frontend_error(
                            source_name,
                            format!(
                                "annotation on {} is invalid: {message}",
                                binding.value.name.value
                            ),
                        )
                    })?;
                    if !contains_named_type(&expected)
                        && !assignable(&inferred, &expected)
                        && !is_declared_literal_construction(
                            &binding.value.value,
                            &inferred,
                            &expected,
                        )
                    {
                        let message = format!(
                            "binding {} has type {}, which is not assignable to {}",
                            binding.value.name.value,
                            inferred.display_name(),
                            expected.display_name()
                        );
                        {
                            let path =
                                incompatibility_path(&inferred, &expected).unwrap_or_default();
                            let data_span = match &binding.value.value.value {
                                ExprKind::Variable(name) => external_provenance
                                    .get(&name.value)
                                    .and_then(|provenance| {
                                        provenance
                                            .values
                                            .get(&path)
                                            .or_else(|| provenance.values.get(&Vec::new()))
                                    })
                                    .cloned(),
                                _ => expression_location_at_path(&binding.value.value, &path)
                                    .or(Some(binding.value.value.location)),
                            }
                            .unwrap_or(binding.location);
                            let rule_span = match &annotation.value {
                                ExprKind::Variable(name) => {
                                    declared_type_spans.get(&name.value).copied()
                                }
                                _ => Some(annotation.location),
                            }
                            .unwrap_or(binding.location);
                            let diagnostic = Diagnostic::error(message, data_span)
                                .with_secondary("type requirement declared here", rule_span);
                            return Err(FrontendError::from_diagnostic(sources, diagnostic));
                        }
                    }
                    expected
                } else {
                    inferred
                };
                static_environment.insert(binding.value.name.value.clone(), checked.clone());
                binding_types.insert(binding.value.name.value.clone(), checked);

                if let Ok(value) = evaluate_tool_expression(
                    source_name,
                    &binding.value.value,
                    &tool_values,
                    account,
                    sources,
                    &mut evaluator,
                ) {
                    tool_values.insert(binding.value.name.value.clone(), value);
                }
            }
            BindingKind::Def => {
                let name = &binding.value.name.value;
                let inferred = inferred_expression;
                let checked = definition_contracts
                    .get(name)
                    .map(erase_type_variables)
                    .unwrap_or(inferred);
                static_environment.insert(name.clone(), checked.clone());
                binding_types.insert(name.clone(), checked);
                if let Ok(value) = evaluate_tool_expression(
                    source_name,
                    &binding.value.value,
                    &tool_values,
                    account,
                    sources,
                    &mut evaluator,
                ) {
                    tool_values.insert(name.clone(), value);
                }
            }
            BindingKind::Import => {
                let value = external_values
                    .get(&binding.value.name.value)
                    .cloned()
                    .ok_or_else(|| {
                        frontend_error(
                            source_name,
                            format!("import {} has not been resolved", binding.value.name.value),
                        )
                    })?;
                let interface = qualified_external_interfaces.get(&binding.value.name.value);
                let scheme = interface
                    .and_then(|interface| interface.exports.get(&binding.value.name.value))
                    .cloned();
                let inferred =
                    imported_static_descriptor(&value, interface, &binding.value.name.value);
                static_environment.insert(binding.value.name.value.clone(), inferred.clone());
                binding_types.insert(binding.value.name.value.clone(), inferred);
                if let Some(scheme) = scheme {
                    let tool_value = authoritative_imported_value(
                        &value,
                        interface,
                        &binding.value.name.value,
                        &mut tool_vm,
                    );
                    binding_schemes.insert(binding.value.name.value.clone(), scheme);
                    tool_values.insert(
                        binding.value.name.value.clone(),
                        evaluator.publish(&tool_value)?,
                    );
                } else {
                    let tool_value = authoritative_imported_value(
                        &value,
                        interface,
                        &binding.value.name.value,
                        &mut tool_vm,
                    );
                    tool_values.insert(
                        binding.value.name.value.clone(),
                        evaluator.publish(&tool_value)?,
                    );
                }
            }
        }
    }

    for (name, location) in &declaration_locations {
        if definition_counts.get(name).copied().unwrap_or(0) == 0 {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!("definition {name:?} was declared but never initialized"),
                    *location,
                ),
            ));
        }
    }

    check_interpolations(
        &program.value.body.value.result,
        &static_environment,
        sources,
    )?;
    infer_expr_recorded(
        &program.value.body.value.result,
        &static_environment,
        &mut expression_descriptors,
    );
    let mut local_annotations = HashMap::new();
    for binding in &program.value.body.value.bindings {
        let mut annotation_values = tool_values.clone();
        for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
            let value = TypeDescriptor::Bound(TypeParameterId(index as u32)).to_value(&mut tool_vm);
            annotation_values.insert(parameter.value.clone(), evaluator.publish(&value)?);
        }
        collect_nested_annotation_types(
            source_name,
            &binding.value.value,
            &annotation_values,
            account,
            sources,
            &mut evaluator,
            &mut local_annotations,
        )?;
    }
    collect_nested_annotation_types(
        source_name,
        &program.value.body.value.result,
        &tool_values,
        account,
        sources,
        &mut evaluator,
        &mut local_annotations,
    )?;
    let mut named_types = imported_named_types;
    named_types.extend(declared_types.clone());
    let mut inference = GenericInference::new(
        &binding_schemes,
        &hir,
        &qualified_external_interfaces,
        &named_types,
        &local_annotations,
        !external_values.contains_key("Tuple"),
        account.query_context(),
    );
    let mut checked_environment = static_environment.clone();
    let type_metadata_expected = TypeDescriptor::Type;
    let mut delayed_bindings = Vec::new();
    let mut recursive_skeletons = HashMap::new();
    let component_plan = definition_component_plan(&program.value.body, &hir);
    if let Some(location) = component_plan.indirect_recursive.iter().next() {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                "indirect recursive definition requires an explicit contract",
                *location,
            ),
        ));
    }
    let uncontracted_definition_names = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| {
            binding.value.kind == BindingKind::Def
                && binding.value.annotation.is_none()
                && !definition_contracts.contains_key(&binding.value.name.value)
        })
        .map(|binding| binding.value.name.value.clone())
        .collect::<HashSet<_>>();
    for binding in &program.value.body.value.bindings {
        if binding.value.kind != BindingKind::Def
            || binding.value.annotation.is_some()
            || definition_contracts.contains_key(&binding.value.name.value)
            || !component_plan
                .recursive
                .contains(&binding.value.name.location)
        {
            continue;
        }
        let first_owned_variable = inference.next_variable;
        if let Some(skeleton) = inference.recursive_closure_skeleton(&binding.value.value) {
            checked_environment.insert(binding.value.name.value.clone(), skeleton.clone());
            inference.set_local_scheme(binding.value.name.value.clone(), None);
            recursive_skeletons.insert(
                binding.value.name.value.clone(),
                (skeleton.clone(), first_owned_variable),
            );
            delayed_bindings.push((
                binding.value.name.value.clone(),
                binding.value.value.location,
                skeleton,
                first_owned_variable,
            ));
        }
    }
    let recursive_variables = recursive_skeletons
        .values()
        .filter_map(|(skeleton, _)| GenericInference::recursive_result_variable(skeleton))
        .collect::<HashSet<_>>();
    for binding in &program.value.body.value.bindings {
        let Some((skeleton, _)) = recursive_skeletons.get(&binding.value.name.value) else {
            continue;
        };
        inference.delayed_initializer_depth += 1;
        inference.recursive_body_inference_depth += 1;
        let recursive_expected = GenericInference::recursive_expected(skeleton);
        let inferred = inference.infer(
            &binding.value.value,
            &checked_environment,
            Some(&recursive_expected),
        );
        inference.recursive_body_inference_depth -= 1;
        inference.delayed_initializer_depth -= 1;
        let inferred = inferred.map_err(|message| {
            let location = inference.take_failure_location(binding.value.value.location);
            FrontendError::from_diagnostic(sources, Diagnostic::error(message, location))
        })?;
        if let (
            Some(variable),
            TypeDescriptor::Function {
                result: inferred_result,
                ..
            },
        ) = (
            GenericInference::recursive_result_variable(skeleton),
            inferred,
        ) {
            inference
                .recursive_equations
                .insert(variable, *inferred_result);
        }
        binding_types.insert(binding.value.name.value.clone(), skeleton.clone());
    }
    inference
        .solve_recursive_equations(&recursive_variables)
        .map_err(|message| frontend_error(source_name, message))?;
    for location in &component_plan.acyclic {
        let binding = program
            .value
            .body
            .value
            .bindings
            .iter()
            .find(|binding| binding.value.name.location == *location)
            .expect("component binding exists");
        let first_owned_variable = inference.next_variable;
        inference.delayed_initializer_depth += 1;
        let inferred = inference.infer(&binding.value.value, &checked_environment, None);
        inference.delayed_initializer_depth -= 1;
        let inferred = inferred.map_err(|message| {
            let location = inference.take_failure_location(binding.value.value.location);
            FrontendError::from_diagnostic(sources, Diagnostic::error(message, location))
        })?;
        let scheme = inference
            .generalize_local_closure(&inferred, first_owned_variable, binding.value.name.location)
            .map_err(|message| {
                FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(message, binding.value.value.location),
                )
            })?;
        let descriptor = scheme.as_ref().map_or_else(
            || inference.resolve(&inferred),
            |scheme| scheme.body.clone(),
        );
        checked_environment.insert(binding.value.name.value.clone(), descriptor.clone());
        binding_types.insert(binding.value.name.value.clone(), descriptor);
        inference.set_local_scheme(binding.value.name.value.clone(), scheme.clone());
        if let Some(scheme) = scheme {
            inference
                .inferred_schemes
                .insert(binding.value.name.location, scheme.clone());
            inference
                .top_level_inferred_schemes
                .insert(binding.value.name.value.clone(), scheme);
        } else {
            delayed_bindings.push((
                binding.value.name.value.clone(),
                binding.value.value.location,
                inferred,
                first_owned_variable,
            ));
        }
    }
    for binding in &program.value.body.value.bindings {
        if matches!(
            binding.value.kind,
            BindingKind::Decl
                | BindingKind::Native
                | BindingKind::Import
                | BindingKind::OpenImport
                | BindingKind::Export
        ) {
            if binding.value.kind == BindingKind::Import {
                let scheme = external_interfaces
                    .get(&binding.value.name.value)
                    .and_then(|interface| interface.exports.get(&binding.value.name.value))
                    .cloned();
                inference.set_local_scheme(binding.value.name.value.clone(), scheme);
            }
            continue;
        }
        if recursive_skeletons.contains_key(&binding.value.name.value) {
            continue;
        }
        if component_plan
            .acyclic
            .contains(&binding.value.name.location)
        {
            continue;
        }
        let expected = if binding.value.kind == BindingKind::Type {
            Some(&type_metadata_expected)
        } else {
            definition_contracts
                .get(&binding.value.name.value)
                .or_else(|| {
                    binding
                        .value
                        .annotation
                        .as_ref()
                        .and_then(|_| binding_types.get(&binding.value.name.value))
                })
                .or_else(|| {
                    recursive_skeletons
                        .get(&binding.value.name.value)
                        .map(|(skeleton, _)| skeleton)
                })
        };
        let is_recursive = recursive_skeletons.contains_key(&binding.value.name.value);
        if binding.value.kind == BindingKind::Def
            && binding.value.annotation.is_none()
            && !is_recursive
            && expression_references_names(
                &binding.value.value,
                &uncontracted_definition_names,
                &HashSet::new(),
            )
        {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!(
                        "recursive definition {:?} requires a closure value or explicit contract",
                        binding.value.name.value
                    ),
                    binding.value.value.location,
                ),
            ));
        }
        let is_delayed = (expected.is_none() || is_recursive)
            && matches!(binding.value.kind, BindingKind::Let | BindingKind::Def)
            && !definition_contracts.contains_key(&binding.value.name.value);
        let first_owned_variable = recursive_skeletons
            .get(&binding.value.name.value)
            .map_or(inference.next_variable, |(_, first)| *first);
        if is_delayed {
            inference.delayed_initializer_depth += 1;
        }
        let mut initializer_environment;
        let environment = if is_delayed && binding.value.kind == BindingKind::Def && !is_recursive {
            initializer_environment = checked_environment.clone();
            initializer_environment.remove(&binding.value.name.value);
            &initializer_environment
        } else {
            &checked_environment
        };
        let inferred = inference.infer(&binding.value.value, environment, expected);
        if is_delayed {
            inference.delayed_initializer_depth -= 1;
        }
        let inferred = inferred.map_err(|message| {
            let location = inference.take_failure_location(binding.value.value.location);
            FrontendError::from_diagnostic(sources, Diagnostic::error(message, location))
        })?;
        if binding.value.kind == BindingKind::Type {
            continue;
        }
        if matches!(binding.value.kind, BindingKind::Let | BindingKind::Def) {
            let inferred_scheme = if binding.value.kind == BindingKind::Let
                && binding.value.annotation.is_none()
                && binding.value.type_parameters.is_empty()
                && matches!(binding.value.value.value, ExprKind::Closure { .. })
            {
                inference
                    .generalize_local_closure(
                        &inferred,
                        first_owned_variable,
                        binding.value.name.location,
                    )
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(message, binding.value.value.location),
                        )
                    })?
            } else {
                None
            };
            let checked = inferred_scheme.as_ref().map_or_else(
                || expected.cloned().unwrap_or(inferred),
                |scheme| scheme.body.clone(),
            );
            checked_environment.insert(binding.value.name.value.clone(), checked.clone());
            binding_types.insert(binding.value.name.value.clone(), checked.clone());
            if inferred_scheme.is_some()
                || binding.value.kind == BindingKind::Let
                || binding.value.annotation.is_none()
                    && !definition_contracts.contains_key(&binding.value.name.value)
            {
                inference
                    .set_local_scheme(binding.value.name.value.clone(), inferred_scheme.clone());
            }
            if let Some(scheme) = &inferred_scheme {
                inference
                    .inferred_schemes
                    .insert(binding.value.name.location, scheme.clone());
            }
            if let Some(scheme) = inferred_scheme {
                inference
                    .top_level_inferred_schemes
                    .insert(binding.value.name.value.clone(), scheme);
            } else if is_delayed && !is_recursive {
                delayed_bindings.push((
                    binding.value.name.value.clone(),
                    binding.value.value.location,
                    checked,
                    first_owned_variable,
                ));
            }
        }
    }
    let result_type = inference
        .infer(&program.value.body.value.result, &checked_environment, None)
        .map_err(|message| {
            let location =
                inference.take_failure_location(program.value.body.value.result.location);
            FrontendError::from_diagnostic(sources, Diagnostic::error(message, location))
        })?;
    let module_requirement = inference
        .propagation_boundaries
        .pop()
        .expect("module propagation boundary exists");
    let result_type = inference
        .finish_propagation_boundary(result_type, None, module_requirement)
        .map_err(|message| {
            FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(message, program.value.body.value.result.location),
            )
        })?;
    if let Some((location, message)) = inference.pattern_diagnostics.first_key_value() {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(message.clone(), *location),
        ));
    }
    if let Some((location, message)) = inference.unresolved_placeholder_since(0) {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(message, location),
        ));
    }
    for (name, location, descriptor, first_owned_variable) in delayed_bindings {
        if let Some(query) = &inference.query {
            query.check().map_err(|error| {
                FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(error.to_string(), location),
                )
            })?;
        }
        let resolved = inference.resolve(&descriptor);
        if contains_inference_variable_at_or_after(&resolved, first_owned_variable) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!(
                        "cannot infer monomorphic binding {name:?}: unresolved {}",
                        resolved.display_name()
                    ),
                    location,
                ),
            ));
        }
    }
    expression_descriptors.extend(
        inference
            .records
            .iter()
            .map(|(location, ty)| (*location, inference.resolve(ty))),
    );
    inference.top_level_inferred_schemes = inference
        .top_level_inferred_schemes
        .iter()
        .map(|(name, scheme)| {
            let mut scheme = scheme.clone();
            scheme.body = inference.resolve(&scheme.body);
            (name.clone(), scheme)
        })
        .collect();
    inference.inferred_schemes = inference
        .inferred_schemes
        .iter()
        .map(|(location, scheme)| {
            let mut scheme = scheme.clone();
            scheme.body = inference.resolve(&scheme.body);
            (*location, scheme)
        })
        .collect();
    binding_schemes.extend(inference.top_level_inferred_schemes.clone());
    let explicitly_exported_locals = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| binding.value.kind == BindingKind::Export)
        .filter_map(|binding| binding.value.imported_name.as_deref())
        .map(|name| name.value.as_str())
        .collect::<HashSet<_>>();
    for (name, descriptor) in &binding_types {
        if explicitly_exported_locals.contains(name.as_str()) {
            binding_schemes
                .entry(name.clone())
                .or_insert_with(|| TypeScheme {
                    parameters: Vec::new(),
                    body: inference.resolve(descriptor),
                });
        }
    }
    let resolved_result = inference.resolve(&result_type);
    for (name, descriptor) in &binding_types {
        let resolved = inference.resolve(descriptor);
        if contains_type_variable(&resolved) {
            return Err(frontend_error(
                source_name,
                format!(
                    "cannot publish unresolved binding {name:?}: {}",
                    resolved.display_name()
                ),
            ));
        }
    }
    for (name, scheme) in &inference.top_level_inferred_schemes {
        validate_publishable_scheme(scheme).map_err(|message| {
            frontend_error(
                source_name,
                format!("cannot publish scheme for {name:?}: {message}"),
            )
        })?;
    }
    let mut types = TypeGraph::default();
    let declared_type_names = declared_types.keys().cloned().collect::<Vec<_>>();
    let installed_named_types = types.install_named_descriptors(&named_types);
    let declared_types = declared_type_names
        .into_iter()
        .map(|name| (name.clone(), installed_named_types[&name]))
        .collect::<BTreeMap<_, _>>();
    let binding_types: BTreeMap<String, TypeId> = binding_types
        .into_iter()
        .map(|(name, descriptor)| {
            let descriptor = inference.resolve(&descriptor);
            (name, types.intern_erased_descriptor(&descriptor))
        })
        .collect();
    let result_type = types.intern_erased_descriptor(&resolved_result);
    let expression_types: BTreeMap<HirExpressionId, TypeId> = hir
        .expressions()
        .iter()
        .filter_map(|expression| {
            expression_descriptors
                .get(&expression.location)
                .map(|descriptor| (expression.id, types.intern_erased_descriptor(descriptor)))
        })
        .collect();
    let any_type = types.intern_descriptor(&TypeDescriptor::Any);
    let pattern_definition_types = hir
        .definitions()
        .iter()
        .filter(|definition| definition.kind == HirDefinitionKind::Pattern)
        .filter_map(|definition| {
            inference
                .pattern_binding_types
                .get(&definition.location)
                .map(|descriptor| {
                    (
                        definition.id,
                        types.intern_erased_descriptor(&inference.resolve(descriptor)),
                    )
                })
        })
        .collect::<HashMap<_, _>>();
    let definition_types = hir
        .definitions()
        .iter()
        .map(|definition| {
            let ty = if definition.top_level {
                binding_types.get(&definition.name).copied()
            } else {
                definition
                    .value
                    .and_then(|value| expression_types.get(&value).copied())
            }
            .or_else(|| pattern_definition_types.get(&definition.id).copied())
            .unwrap_or(any_type);
            (definition.id, ty)
        })
        .collect();
    let definition_schemes = hir
        .definitions()
        .iter()
        .filter_map(|definition| {
            inference
                .inferred_schemes
                .get(&definition.location)
                .cloned()
                .or_else(|| {
                    definition
                        .top_level
                        .then(|| binding_schemes.get(&definition.name))
                        .flatten()
                        .filter(|scheme| !scheme.parameters.is_empty())
                        .cloned()
                })
                .map(|scheme| (definition.id, scheme))
        })
        .collect::<BTreeMap<_, _>>();
    for (definition, scheme) in &definition_schemes {
        if hir
            .definition(*definition)
            .is_some_and(|definition| definition.top_level)
        {
            validate_publishable_scheme(scheme)
                .map_err(|message| frontend_error(source_name, message))?;
        }
    }
    let module_interface = ModuleInterface {
        exports: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields
                .iter()
                .filter_map(|field| {
                    let ExprKind::Variable(binding) = &field.value.value.value else {
                        return None;
                    };
                    binding_schemes
                        .get(&binding.value)
                        .cloned()
                        .and_then(|scheme| {
                            field
                                .value
                                .name
                                .as_ref()
                                .map(|name| (name.value.clone(), scheme))
                        })
                })
                .collect(),
            _ => BTreeMap::new(),
        },
        concrete_types: named_types
            .iter()
            .filter(|(_, descriptor)| contains_named_type(descriptor))
            .map(|(name, descriptor)| (name.clone(), descriptor.clone()))
            .collect(),
        type_family_templates: match &program.value.body.value.result.value {
            ExprKind::Dict(fields) => fields
                .iter()
                .filter_map(|field| {
                    let ExprKind::Variable(binding) = &field.value.value.value else {
                        return None;
                    };
                    field.value.name.as_ref().and_then(|name| {
                        type_family_templates
                            .get(&binding.value)
                            .cloned()
                            .or_else(|| {
                                qualified_external_interfaces
                                    .get(&binding.value)
                                    .and_then(|interface| {
                                        interface.type_family_templates.get(&binding.value)
                                    })
                                    .cloned()
                            })
                            .map(|family| (name.value.clone(), family))
                    })
                })
                .collect(),
            _ => BTreeMap::new(),
        },
    };
    for scheme in module_interface.exports.values() {
        validate_publishable_scheme(scheme)
            .map_err(|message| frontend_error(source_name, message))?;
    }
    let propagation_families = std::mem::take(&mut inference.propagation_families);
    let not_families = std::mem::take(&mut inference.not_families);
    let declared_value_owners = expression_descriptors
        .iter()
        .filter(|(_, descriptor)| matches!(descriptor, TypeDescriptor::Declared(_)))
        .map(|(location, descriptor)| (*location, descriptor.to_value(&mut tool_vm)))
        .collect();
    Ok(Analysis {
        types,
        declared_types,
        binding_types,
        result_type,
        hir,
        definition_types,
        definition_schemes,
        expression_types,
        module_interface,
        explicit_exports: program
            .value
            .body
            .value
            .bindings
            .iter()
            .any(|binding| binding.value.kind == BindingKind::Export),
        propagation_families,
        not_families,
        prelude: prelude_values,
        external_values: external_values.clone(),
        dynamic_bindings: dynamic_bindings.clone(),
        type_family_values,
        declared_value_owners,
    })
}

fn authoritative_imported_metadata(
    value: &Value,
    scheme: Option<&TypeScheme>,
    vm: &mut Vm,
) -> Value {
    let Some(TypeDescriptor::TypeOf(expected)) = scheme.map(|scheme| &scheme.body) else {
        return value.clone();
    };
    match TypeDescriptor::from_value(value) {
        Ok(actual) if actual == **expected => value.clone(),
        _ => expected.to_value(vm),
    }
}

fn imported_static_descriptor(
    value: &Value,
    interface: Option<&ModuleInterface>,
    local: &str,
) -> TypeDescriptor {
    let Some(interface) = interface.filter(|interface| !interface.exports.is_empty()) else {
        return infer_value(value);
    };
    if let Some(scheme) = interface.exports.get(local) {
        return erase_type_variables(&scheme.body);
    }
    let mut fields = match infer_value(value) {
        TypeDescriptor::Struct(fields) => fields,
        _ => BTreeMap::new(),
    };
    for (name, scheme) in &interface.exports {
        fields.insert(name.clone(), erase_type_variables(&scheme.body));
    }
    TypeDescriptor::Struct(fields)
}

fn authoritative_imported_value(
    value: &Value,
    interface: Option<&ModuleInterface>,
    local: &str,
    vm: &mut Vm,
) -> Value {
    let Some(interface) = interface else {
        return value.clone();
    };
    if let Some(family) = interface.type_family_templates.get(local) {
        return type_family_value(family);
    }
    if let Some(scheme) = interface.exports.get(local) {
        return authoritative_imported_metadata(value, Some(scheme), vm);
    }
    let Value::Dict(fields) = value else {
        return value.clone();
    };
    let values = fields
        .shape()
        .fields()
        .iter()
        .zip(fields.values())
        .map(|(name, value)| {
            if let Some(family) = interface.type_family_templates.get(name) {
                type_family_value(family)
            } else {
                authoritative_imported_metadata(value, interface.exports.get(name), vm)
            }
        })
        .collect();
    Value::Dict(crate::Dict::new(fields.shape().clone(), values))
}

fn validate_interpreter_contract(
    type_parameters: &[crate::ast::Identifier],
    contract: Option<&TypeDescriptor>,
) -> Result<(), String> {
    if contract.is_none() {
        return Err(
            "interpreter requires an explicit for(A, ...) Fn(TypeOf(A), ...) -> Fn(...) -> R definition contract"
                .into(),
        );
    }
    if type_parameters.is_empty() {
        return Err("interpreter requires at least one quantified type parameter".into());
    }
    let Some(TypeDescriptor::Function {
        parameters: outer_parameters,
        result: outer_result,
    }) = contract
    else {
        return Err(
            "interpreter contract must return an inner Function from explicit TypeOf witnesses"
                .into(),
        );
    };

    let mut witnesses = HashMap::new();
    for (index, witness) in outer_parameters.iter().enumerate() {
        let TypeDescriptor::TypeOf(parameter) = witness else {
            return Err(format!(
                "interpreter witness parameter {} must have type TypeOf(A)",
                index + 1
            ));
        };
        let TypeDescriptor::Bound(parameter) = parameter.as_ref() else {
            return Err(format!(
                "interpreter witness parameter {} must name a quantified type parameter",
                index + 1
            ));
        };
        let Some(name) = type_parameters.get(parameter.0 as usize) else {
            return Err("interpreter witness refers to an unknown type parameter".into());
        };
        if witnesses.insert(*parameter, index).is_some() {
            return Err(format!(
                "interpreter type parameter {} has more than one TypeOf witness",
                name.value
            ));
        }
    }
    for (index, parameter) in type_parameters.iter().enumerate() {
        if !witnesses.contains_key(&TypeParameterId(index as u32)) {
            return Err(format!(
                "interpreter type parameter {} has no TypeOf witness",
                parameter.value
            ));
        }
    }

    let TypeDescriptor::Function {
        parameters: inner_parameters,
        result,
    } = outer_result.as_ref()
    else {
        return Err("interpreter TypeOf witnesses must return an inner Function".into());
    };
    let interpreted = witnesses.keys().copied().collect::<HashSet<_>>();
    for (index, parameter) in inner_parameters.iter().enumerate() {
        if let TypeDescriptor::Bound(bound) = parameter
            && interpreted.contains(bound)
        {
            continue;
        }
        let mut mentioned = Vec::new();
        collect_bound_parameters(parameter, &mut mentioned);
        if let Some(bound) = mentioned
            .into_iter()
            .find(|bound| interpreted.contains(bound))
        {
            let name = &type_parameters[bound.0 as usize].value;
            return Err(format!(
                "interpreter inner parameter {} contains type parameter {}; only a direct {} parameter can be lifted",
                index + 1,
                name,
                name
            ));
        }
    }
    let mut result_parameters = Vec::new();
    collect_bound_parameters(result, &mut result_parameters);
    if let Some(bound) = result_parameters
        .into_iter()
        .find(|bound| interpreted.contains(bound))
    {
        return Err(format!(
            "interpreter result contains type parameter {}; lifted interpreters cannot return interpreted values",
            type_parameters[bound.0 as usize].value
        ));
    }
    Ok(())
}

pub(crate) fn infer_value(value: &Value) -> TypeDescriptor {
    match value {
        Value::Int(_) => TypeDescriptor::Int,
        Value::Float(_) => TypeDescriptor::Float,
        Value::String(_) => TypeDescriptor::String,
        Value::Bytes(_) => TypeDescriptor::Bytes,
        Value::NativeType(native_type) => {
            TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Opaque(native_type.clone())))
        }
        Value::DeclaredType(value) => TypeDescriptor::TypeOf(Box::new(
            TypeDescriptor::from_value(&Value::DeclaredType(value.clone()))
                .unwrap_or(TypeDescriptor::Any),
        )),
        Value::Declared(value) => {
            TypeDescriptor::from_value(&Value::DeclaredType(value.owner().clone()))
                .unwrap_or_else(|_| infer_value(value.payload()))
        }
        Value::Opaque(value) => TypeDescriptor::Opaque(value.native_type().clone()),
        Value::Atom(atom) => TypeDescriptor::Atom(atom.clone()),
        Value::Array(items) => {
            let item =
                common_type(items.iter().map(infer_value).collect()).unwrap_or(TypeDescriptor::Any);
            TypeDescriptor::Array(Box::new(item))
        }
        Value::Tagged { tag, payload } => TypeDescriptor::Tagged {
            tag: tag.clone(),
            payload: Box::new(infer_value(payload)),
        },
        Value::Tuple(items) => TypeDescriptor::Tuple(items.iter().map(infer_value).collect()),
        Value::Dict(fields) => TypeDescriptor::Struct(
            fields
                .shape()
                .fields()
                .iter()
                .zip(fields.values())
                .map(|(name, value)| (name.clone(), infer_value(value)))
                .collect(),
        ),
        Value::Func(closure) => {
            let arity = match closure.prototype() {
                crate::Prototype::Bytecode(function) => function.parameter_count(),
                crate::Prototype::Native(function) => function.arity(),
            };
            TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Any; arity],
                result: Box::new(TypeDescriptor::Any),
            }
        }
        Value::Dyn(_) => TypeDescriptor::Dyn,
    }
}

fn declare_metadata_value(
    source_name: &str,
    binding: &Binding,
    slots: &HashMap<crate::Location, u32>,
    value: PersistentValue,
    evaluator: &mut ToolEvaluator,
) -> Result<PersistentValue, FrontendError> {
    let Some(kind) = binding.value.declared_initializer else {
        return Ok(value);
    };
    let descriptor = evaluator.decode_type(value, "Type").map_err(|message| {
        frontend_error(
            source_name,
            format!(
                "declared type {} produced invalid metadata: {message}",
                binding.value.name.value
            ),
        )
    })?;
    let valid = matches!(
        (kind, &descriptor),
        (
            crate::ast::DeclaredInitializerKind::Struct,
            TypeDescriptor::Struct(_)
        ) | (
            crate::ast::DeclaredInitializerKind::Enum,
            TypeDescriptor::Enum(_)
        )
    );
    if !valid {
        return Err(frontend_error(
            source_name,
            format!(
                "declared type {} initializer changed its root model kind",
                binding.value.name.value
            ),
        ));
    }
    let slot = slots
        .get(&binding.value.name.location)
        .copied()
        .expect("direct declared initializer has a declaration slot");
    evaluator
        .heap
        .declare_persistent_type(value, source_name, slot, binding.value.name.value.as_str())
        .map_err(|error| frontend_error(source_name, error.to_string()))
}

fn is_declared_literal_construction(
    expression: &Expr,
    _actual: &TypeDescriptor,
    expected: &TypeDescriptor,
) -> bool {
    match (expected, &expression.value) {
        (TypeDescriptor::Declared(declared), _) => {
            declared_body_accepts_expression(&declared.body, expression)
        }
        (TypeDescriptor::Array(item), ExprKind::Array(items))
            if matches!(item.as_ref(), TypeDescriptor::Declared(_)) =>
        {
            items.iter().all(|item_expression| {
                !matches!(item_expression.value, ExprKind::Spread(_))
                    && is_declared_literal_construction(item_expression, &TypeDescriptor::Any, item)
            })
        }
        _ => false,
    }
}

fn declared_body_accepts_expression(body: &TypeDescriptor, expression: &Expr) -> bool {
    match (body, &expression.value) {
        (TypeDescriptor::Struct(_), ExprKind::Dict(_))
        | (TypeDescriptor::Enum(_), ExprKind::Atom(_)) => true,
        (TypeDescriptor::Enum(_), ExprKind::Call { callee, .. }) => {
            matches!(callee.value, ExprKind::Atom(_))
        }
        _ => false,
    }
}

fn evaluate_tool_expression(
    source_name: &str,
    expression: &Expr,
    bindings: &BTreeMap<String, PersistentValue>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
) -> Result<PersistentValue, FrontendError> {
    let function = compile_expression_with_external_bindings(
        source_name,
        "<tool-stage>",
        expression,
        bindings.keys().cloned(),
        sources.get(expression.location.source),
    )?;
    let externals = bindings
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect::<HashMap<_, _>>();
    let world = evaluator
        .vm
        .execute_in_work(&evaluator.heap, &externals, &function, &[], account)
        .map_err(|error| {
            frontend_error(
                source_name,
                format!(
                    "tool-stage evaluation failed: {}",
                    error.with_sources(sources)
                ),
            )
        })?;
    world.publish(&mut evaluator.heap).map_err(|error| {
        frontend_error(
            source_name,
            format!("tool-stage publication failed: {error}"),
        )
    })
}

struct ToolEvaluator {
    vm: Vm,
    heap: Heap,
}

impl ToolEvaluator {
    fn new(debug_sink: Arc<dyn DebugSink>) -> Self {
        Self {
            vm: Vm::new().with_debug_sink(debug_sink),
            heap: Heap::main(),
        }
    }

    fn publish(&mut self, value: &Value) -> Result<PersistentValue, FrontendError> {
        crate::heap::publish_value(&mut self.heap, value)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }

    fn publish_map(
        &mut self,
        values: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, PersistentValue>, FrontendError> {
        values
            .iter()
            .map(|(name, value)| self.publish(value).map(|value| (name.clone(), value)))
            .collect()
    }

    fn decode_type(&self, value: PersistentValue, path: &str) -> Result<TypeDescriptor, String> {
        decode_type_ref(ValueRef::persistent(value, &self.heap), path)
    }

    fn export(&self, value: PersistentValue) -> Result<Value, FrontendError> {
        self.heap
            .export_persistent(value)
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }
}

pub(crate) fn type_family_value(family: &TypeFamilyTemplate) -> Value {
    let arity = family.parameters.len();
    let arity_value = i64::try_from(arity).expect("type-family arity was already bounded by u32");
    Value::Func(Arc::new(Closure::native_with_upvalues(
        NativeFunction::new("type-family.apply", arity, native_apply_type_family),
        vec![family.metadata.clone(), Value::Int(arity_value)],
    )))
}

fn native_apply_type_family(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let template = context.upvalue(0)?;
    let arity = context
        .value(context.upvalue(1)?)?
        .as_int()
        .and_then(|arity| usize::try_from(arity).ok())
        .ok_or_else(|| NativeError::new("invalid type-family arity"))?;
    let mut argument_registers = Vec::with_capacity(arity);
    let mut argument_descriptors = Vec::with_capacity(arity);
    for index in 0..arity {
        let register = context.argument(index)?;
        let argument = native_type_argument_descriptor(context, register, index)?;
        argument_registers.push(register);
        argument_descriptors.push(argument);
    }
    let result = context.result();
    context.instantiate_type_family(
        result,
        template,
        &argument_registers,
        &argument_descriptors,
    )?;
    context.mark_at_call_site(result)
}

pub(crate) fn native_declare_type_family(
    context: &mut CallContext<'_, '_>,
) -> Result<(), NativeError> {
    let template = context.argument(0)?;
    let body = context.argument(1)?;
    let (head, name, _) = context
        .value(template)?
        .declared_type_parts()
        .ok_or_else(|| NativeError::new("type-family declaration template is not declared"))?;
    let head = head.clone();
    let name = name.to_owned();
    let arity = context.argument_count().saturating_sub(2);
    let mut argument_registers = Vec::with_capacity(arity);
    let mut argument_descriptors = Vec::with_capacity(arity);
    for index in 0..arity {
        let register = context.argument(index + 2)?;
        let argument = native_type_argument_descriptor(context, register, index)?;
        argument_registers.push(register);
        argument_descriptors.push(argument);
    }
    let id = apply_declared_type_arguments(&head, &argument_descriptors);
    context.make_declared_type_application(
        context.result(),
        id,
        name,
        body,
        &argument_registers,
    )?;
    context.mark_at_call_site(context.result())
}

fn native_type_argument_descriptor(
    context: &CallContext<'_, '_>,
    register: RegisterId,
    index: usize,
) -> Result<TypeDescriptor, NativeError> {
    validate_native_type(context.value(register)?).map_err(|error| {
        NativeError::new(format!(
            "type-family argument {} is not valid TypeMetadata: {}",
            index + 1,
            error.message
        ))
    })?;
    let is_cyclic = |error: &NativeError| {
        error
            .message
            .contains("cyclic heap values cannot cross the legacy Value boundary")
    };
    let value = match context.export_value(register) {
        Ok(value) => Some(value),
        Err(error) if is_cyclic(&error) => match context.export_type_identity(register) {
            Ok(value) => Some(value),
            Err(error) if is_cyclic(&error) => None,
            Err(error) => return Err(error),
        },
        Err(error) => return Err(error),
    };
    value.map_or(Ok(TypeDescriptor::Any), |value| {
        TypeDescriptor::from_value(&value).map_err(|message| {
            NativeError::new(format!(
                "invalid type-family argument {}: {message}",
                index + 1
            ))
        })
    })
}

fn optional_type_metadata_is_none(metadata: &Value) -> bool {
    if matches!(metadata, Value::Atom(atom) if atom.name() == "None") {
        return true;
    }
    matches!(
        metadata,
        Value::Dict(fields)
            if matches!(fields.get("kind"), Some(Value::Atom(kind)) if kind.name() == "WithAttributes")
                && matches!(fields.get("inner"), Some(Value::Atom(inner)) if inner.name() == "None")
    )
}

#[allow(clippy::too_many_arguments)]
fn collect_nested_annotation_types(
    source_name: &str,
    expression: &Expr,
    bindings: &BTreeMap<String, PersistentValue>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    debug_sink: &mut ToolEvaluator,
    annotations: &mut HashMap<crate::Location, TypeDescriptor>,
) -> Result<(), FrontendError> {
    match &expression.value {
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    collect_nested_annotation_types(
                        source_name,
                        expression,
                        bindings,
                        account,
                        sources,
                        debug_sink,
                        annotations,
                    )?;
                }
            }
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                collect_nested_annotation_types(
                    source_name,
                    item,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Spread(operand) => collect_nested_annotation_types(
            source_name,
            operand,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Dict(fields) => {
            for field in fields {
                collect_nested_annotation_types(
                    source_name,
                    &field.value.value,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Block(block) => {
            collect_block_annotation_types(
                source_name,
                block,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
        }
        ExprKind::Closure {
            parameters,
            result_annotation,
            body,
        } => {
            for annotation in parameters
                .iter()
                .filter_map(|parameter| parameter.annotation.as_ref())
                .chain(result_annotation.as_deref())
            {
                let metadata = evaluate_tool_expression(
                    source_name,
                    annotation,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                )?;
                let descriptor = debug_sink
                    .decode_type(metadata, "Type")
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!("closure annotation is invalid: {message}"),
                                annotation.location,
                            ),
                        )
                    })?;
                annotations.insert(annotation.location, descriptor);
            }
            collect_block_annotation_types(
                source_name,
                body,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
        }
        ExprKind::Unary { operand, .. }
        | ExprKind::Propagate { operand }
        | ExprKind::Field {
            receiver: operand, ..
        }
        | ExprKind::TupleProjection {
            receiver: operand, ..
        } => {
            collect_nested_annotation_types(
                source_name,
                operand,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
        }
        ExprKind::Return { value } => collect_nested_annotation_types(
            source_name,
            value,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Panic { message } => collect_nested_annotation_types(
            source_name,
            message,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Raise { error } => collect_nested_annotation_types(
            source_name,
            error,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Debug { value, .. } => collect_nested_annotation_types(
            source_name,
            value,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::Binary { left, right, .. } => {
            for expression in [left.as_ref(), right.as_ref()] {
                collect_nested_annotation_types(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Index { receiver, index } => {
            for expression in [receiver.as_ref(), index.as_ref()] {
                collect_nested_annotation_types(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Call { callee, arguments } => {
            collect_nested_annotation_types(
                source_name,
                callee,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for argument in arguments {
                collect_nested_annotation_types(
                    source_name,
                    argument,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::TypeApply { callee, arguments } => {
            collect_nested_annotation_types(
                source_name,
                callee,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for argument in arguments {
                let TypeArgumentKind::Explicit(expression) = &argument.value else {
                    continue;
                };
                let metadata = evaluate_tool_expression(
                    source_name,
                    expression,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                )?;
                let descriptor = debug_sink
                    .decode_type(metadata, "Type")
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!("type argument is invalid: {message}"),
                                expression.location,
                            ),
                        )
                    })?;
                annotations.insert(expression.location, descriptor);
            }
        }
        ExprKind::Interpreter { operand, .. } => collect_nested_annotation_types(
            source_name,
            operand,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?,
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_nested_annotation_types(
                source_name,
                condition,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for block in [then_branch, else_branch] {
                collect_block_annotation_types(
                    source_name,
                    block,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for block in [then_branch, else_branch] {
                collect_block_annotation_types(
                    source_name,
                    block,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for block in [else_branch, body] {
                collect_block_annotation_types(
                    source_name,
                    block,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Match { value, arms } => {
            collect_nested_annotation_types(
                source_name,
                value,
                bindings,
                account,
                sources,
                debug_sink,
                annotations,
            )?;
            for arm in arms {
                if let Some(guard) = &arm.value.guard {
                    collect_nested_annotation_types(
                        source_name,
                        guard,
                        bindings,
                        account,
                        sources,
                        debug_sink,
                        annotations,
                    )?;
                }
                collect_nested_annotation_types(
                    source_name,
                    &arm.value.value,
                    bindings,
                    account,
                    sources,
                    debug_sink,
                    annotations,
                )?;
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_)
        | ExprKind::Variable(_) => {}
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn collect_block_annotation_types(
    source_name: &str,
    block: &Block,
    bindings: &BTreeMap<String, PersistentValue>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    debug_sink: &mut ToolEvaluator,
    annotations: &mut HashMap<crate::Location, TypeDescriptor>,
) -> Result<(), FrontendError> {
    for binding in &block.value.bindings {
        if let Some(annotation) = &binding.value.annotation {
            let metadata = evaluate_tool_expression(
                source_name,
                annotation,
                bindings,
                account,
                sources,
                debug_sink,
            )?;
            let descriptor = debug_sink
                .decode_type(metadata, "Type")
                .map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!(
                                "annotation on {} is invalid: {message}",
                                binding.value.name.value
                            ),
                            annotation.location,
                        ),
                    )
                })?;
            annotations.insert(annotation.location, descriptor);
        }
        collect_nested_annotation_types(
            source_name,
            &binding.value.value,
            bindings,
            account,
            sources,
            debug_sink,
            annotations,
        )?;
    }
    collect_nested_annotation_types(
        source_name,
        &block.value.result,
        bindings,
        account,
        sources,
        debug_sink,
        annotations,
    )
}

struct BootstrapPrelude {
    values: BTreeMap<String, Value>,
    types: HashMap<String, TypeDescriptor>,
    schemes: HashMap<String, TypeScheme>,
}

impl BootstrapPrelude {
    fn new(vm: &mut Vm) -> Self {
        let artifact = Self {
            values: core_prelude_values(vm),
            types: core_prelude_types(),
            schemes: core_prelude_schemes(),
        };
        debug_assert!(artifact.schemes.keys().all(|name| {
            artifact.values.contains_key(name) && artifact.types.contains_key(name)
        }));
        artifact
    }
}

fn core_prelude_values(vm: &mut Vm) -> BTreeMap<String, Value> {
    let mut prelude = BTreeMap::new();
    for (name, descriptor) in [
        ("Type", TypeDescriptor::Type),
        ("Dyn", TypeDescriptor::Dyn),
        ("Any", TypeDescriptor::Any),
        ("Never", TypeDescriptor::Never),
        ("Int", TypeDescriptor::Int),
        ("Float", TypeDescriptor::Float),
        ("String", TypeDescriptor::String),
        ("Bytes", TypeDescriptor::Bytes),
    ] {
        prelude.insert(name.into(), descriptor.to_value(vm));
    }
    prelude.insert("Bool".into(), normalized_bool_value(vm));
    prelude.insert("BlameError".into(), blame_error_descriptor().to_value(vm));
    for function in [
        NativeFunction::core_model(CoreModelFunction::Struct),
        NativeFunction::core_model(CoreModelFunction::Enum),
        NativeFunction::core_model(CoreModelFunction::Union),
        NativeFunction::core_builtin_type(CoreBuiltinTypeFunction::Option),
        NativeFunction::core_builtin_type(CoreBuiltinTypeFunction::Result),
        NativeFunction::core_builtin_type(CoreBuiltinTypeFunction::FoldControl),
        NativeFunction::new("Atom", 1, native_atom_type),
        NativeFunction::new("Array", 1, native_array_type),
        NativeFunction::new("Dict", 1, native_dict_type),
        NativeFunction::new("TypeOf", 1, native_type_of_type),
        NativeFunction::new("Tagged", 2, native_tagged_type),
        NativeFunction::new("Tuple", 1, native_tuple_type),
        NativeFunction::new("Func", 2, native_function_type),
        NativeFunction::new("validate", 2, native_validate),
        NativeFunction::core_diagnostic(CoreDiagnosticFunction::Warn),
    ] {
        prelude.insert(
            function.name().into(),
            Value::Func(std::sync::Arc::new(Closure::native(function))),
        );
    }
    prelude.insert(
        "\0telora_pack_dyn".into(),
        Value::Func(Arc::new(Closure::native(NativeFunction::core_dyn(
            CoreDynFunction::Pack,
        )))),
    );
    prelude
}

fn core_prelude_types() -> HashMap<String, TypeDescriptor> {
    let metadata = TypeDescriptor::Type;
    let function =
        |parameters: Vec<TypeDescriptor>, result: TypeDescriptor| TypeDescriptor::Function {
            parameters,
            result: Box::new(result),
        };
    let mut prelude = HashMap::new();
    for (name, instance) in [
        ("Type", TypeDescriptor::Type),
        ("Dyn", TypeDescriptor::Dyn),
        ("Any", TypeDescriptor::Any),
        ("Never", TypeDescriptor::Never),
        ("Int", TypeDescriptor::Int),
        ("Float", TypeDescriptor::Float),
        ("String", TypeDescriptor::String),
        ("Bytes", TypeDescriptor::Bytes),
        ("Bool", normalized_bool_descriptor()),
        ("BlameError", blame_error_descriptor()),
    ] {
        prelude.insert(name.into(), TypeDescriptor::TypeOf(Box::new(instance)));
    }
    prelude.insert(
        "Atom".into(),
        function(vec![TypeDescriptor::Any], metadata.clone()),
    );
    prelude.insert(
        "Array".into(),
        function(vec![metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "Dict".into(),
        function(vec![metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "TypeOf".into(),
        function(vec![metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "Tagged".into(),
        function(
            vec![TypeDescriptor::Any, metadata.clone()],
            metadata.clone(),
        ),
    );
    prelude.insert(
        "Tuple".into(),
        function(
            vec![TypeDescriptor::Array(Box::new(metadata.clone()))],
            metadata.clone(),
        ),
    );
    prelude.insert(
        "Func".into(),
        function(
            vec![
                TypeDescriptor::Array(Box::new(metadata.clone())),
                metadata.clone(),
            ],
            metadata.clone(),
        ),
    );
    for name in ["\0telora_struct", "\0telora_enum"] {
        prelude.insert(
            name.into(),
            function(
                vec![TypeDescriptor::Any, TypeDescriptor::Any],
                metadata.clone(),
            ),
        );
    }
    prelude.insert(
        "Union".into(),
        function(vec![TypeDescriptor::Any], metadata.clone()),
    );
    prelude.insert(
        "Option".into(),
        function(vec![metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "Result".into(),
        function(vec![metadata.clone(), metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "FoldControl".into(),
        function(vec![metadata.clone(), metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "validate".into(),
        function(vec![metadata, TypeDescriptor::Any], TypeDescriptor::Any),
    );
    prelude.insert(
        "\0telora_warn".into(),
        function(
            vec![TypeDescriptor::String, TypeDescriptor::Any],
            TypeDescriptor::Atom(Atom::Builtin(BuiltinAtom::None)),
        ),
    );
    prelude.insert(
        "\0telora_pack_dyn".into(),
        function(
            vec![TypeDescriptor::Type, TypeDescriptor::Any],
            TypeDescriptor::Dyn,
        ),
    );
    prelude
}

fn core_prelude_schemes() -> HashMap<String, TypeScheme> {
    let bound = |index| TypeDescriptor::Bound(TypeParameterId(index));
    let witness = |instance| TypeDescriptor::TypeOf(Box::new(instance));
    let function = |parameters, result| TypeDescriptor::Function {
        parameters,
        result: Box::new(result),
    };
    let scheme = |body| TypeScheme {
        parameters: Vec::new(),
        body,
    };
    HashMap::from([
        (
            "Array".into(),
            scheme(function(
                vec![witness(bound(0))],
                witness(TypeDescriptor::Array(Box::new(bound(0)))),
            )),
        ),
        (
            "Dict".into(),
            scheme(function(
                vec![witness(bound(0))],
                witness(TypeDescriptor::Dict(Box::new(bound(0)))),
            )),
        ),
        (
            "TypeOf".into(),
            scheme(function(
                vec![witness(bound(0))],
                witness(witness(bound(0))),
            )),
        ),
        (
            "Option".into(),
            scheme(function(
                vec![witness(bound(0))],
                witness(option_descriptor(bound(0))),
            )),
        ),
        (
            "Result".into(),
            scheme(function(
                vec![witness(bound(0)), witness(bound(1))],
                witness(result_descriptor(bound(0), bound(1))),
            )),
        ),
        (
            "FoldControl".into(),
            scheme(function(
                vec![witness(bound(0)), witness(bound(1))],
                witness(fold_control_descriptor(bound(0), bound(1))),
            )),
        ),
        (
            "validate".into(),
            scheme(function(
                vec![witness(bound(0)), TypeDescriptor::Any],
                result_descriptor(bound(0), blame_error_descriptor()),
            )),
        ),
        (
            "\0telora_warn".into(),
            scheme(function(
                vec![TypeDescriptor::String, TypeDescriptor::Any],
                TypeDescriptor::Atom(Atom::Builtin(BuiltinAtom::None)),
            )),
        ),
    ])
}

pub(crate) fn audit_default_prelude_interface(interface: &ModuleInterface) -> Result<(), String> {
    let expected = ["union", "validate"].into_iter().collect::<BTreeSet<_>>();
    let actual = interface
        .exports
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err("core/prelude must export exactly union and validate".into());
    }
    let bootstrap = core_prelude_schemes();
    let expected_validate = &bootstrap["validate"];
    let declared_validate = &interface.exports["validate"];
    if declared_validate.body != expected_validate.body {
        return Err(format!(
            "core/prelude validate scheme {} differs from bootstrap {}",
            declared_validate.display_name(),
            expected_validate.display_name()
        ));
    }
    Ok(())
}

fn option_descriptor(item: TypeDescriptor) -> TypeDescriptor {
    TypeDescriptor::Enum(BTreeMap::from([
        ("None".into(), None),
        ("Some".into(), Some(Box::new(item))),
    ]))
}

fn option_parts(
    variants: &BTreeMap<String, Option<Box<TypeDescriptor>>>,
) -> Option<&TypeDescriptor> {
    (variants.len() == 2 && variants.get("None").is_some_and(Option::is_none))
        .then(|| variants.get("Some").and_then(Option::as_deref))
        .flatten()
}

fn blame_error_descriptor() -> TypeDescriptor {
    TypeDescriptor::Struct(BTreeMap::from([
        ("data".into(), TypeDescriptor::Any),
        ("message".into(), TypeDescriptor::String),
        ("rule".into(), TypeDescriptor::Any),
    ]))
}

fn result_descriptor(ok: TypeDescriptor, err: TypeDescriptor) -> TypeDescriptor {
    TypeDescriptor::Enum(BTreeMap::from([
        ("Err".into(), Some(Box::new(err))),
        ("Ok".into(), Some(Box::new(ok))),
    ]))
}

fn result_parts(descriptor: &TypeDescriptor) -> Option<(&TypeDescriptor, &TypeDescriptor)> {
    let TypeDescriptor::Enum(variants) = descriptor else {
        return None;
    };
    if variants.len() != 2 {
        return None;
    }
    Some((
        variants.get("Ok")?.as_deref()?,
        variants.get("Err")?.as_deref()?,
    ))
}

fn fold_control_descriptor(state: TypeDescriptor, result: TypeDescriptor) -> TypeDescriptor {
    TypeDescriptor::Enum(BTreeMap::from([
        ("Break".into(), Some(Box::new(result))),
        ("Continue".into(), Some(Box::new(state))),
    ]))
}

fn normalized_bool_value(vm: &mut Vm) -> Value {
    let variants = ["False", "True"]
        .into_iter()
        .map(|name| (name.into(), normalized_legacy_value(vm, Value::none())))
        .collect::<Vec<_>>();
    let variants = vm
        .make_dict(variants)
        .expect("Bool variant names are unique");
    let metadata = vm
        .make_dict(vec![
            ("kind".into(), Value::atom("Enum")),
            ("variants".into(), variants),
        ])
        .expect("Bool metadata fields are unique");
    normalized_legacy_value(vm, metadata)
}

fn normalized_bool_descriptor() -> TypeDescriptor {
    TypeDescriptor::Enum(BTreeMap::from([
        ("False".into(), None),
        ("True".into(), None),
    ]))
}

fn normalized_legacy_value(vm: &mut Vm, inner: Value) -> Value {
    let attributes = vm
        .make_dict(Vec::new())
        .expect("empty attributes are unique");
    vm.make_dict(vec![
        ("attributes".into(), attributes),
        ("inner".into(), inner),
        ("kind".into(), Value::atom("WithAttributes")),
    ])
    .expect("WithAttributes fields are unique")
}

fn native_atom_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let argument = context.argument(0)?;
    let Some(atom) = context.value(argument)?.as_atom() else {
        return Err(NativeError::new("Atom expects an Atom value"));
    };
    let _ = atom_from_name(atom);
    write_native_type_record(context, "Atom", &[("tag", argument)])
}

fn native_array_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let item = context.argument(0)?;
    let value = context.value(item)?;
    if !value.is_hidden_up_link() {
        validate_native_type(value)?;
    }
    write_native_type_record(context, "Array", &[("item", item)])
}

fn native_dict_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let item = context.argument(0)?;
    let value = context.value(item)?;
    if !value.is_hidden_up_link() {
        validate_native_type(value)?;
    }
    write_native_type_record(context, "Dict", &[("item", item)])
}

fn native_type_of_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let instance = context.argument(0)?;
    let value = context.value(instance)?;
    if !value.is_hidden_up_link() {
        validate_native_type(value)?;
    }
    write_native_type_record(context, "TypeOf", &[("instance", instance)])
}

fn native_tagged_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let tag = context.argument(0)?;
    if context.value(tag)?.as_atom().is_none() {
        return Err(NativeError::new("Tagged expects an Atom tag"));
    }
    let payload = context.argument(1)?;
    let value = context.value(payload)?;
    if !value.is_hidden_up_link() {
        validate_native_type(value)?;
    }
    write_native_type_record(context, "Tagged", &[("tag", tag), ("payload", payload)])
}

fn native_tuple_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let value = context.value(context.argument(0)?)?;
    if value.kind() != ValueKind::Array {
        return Err(NativeError::new("Tuple expects an Array of Types"));
    }
    for index in 0..value.sequence_len().expect("Array has a length") {
        let item = value.sequence_get(index).expect("valid Array index");
        if !item.is_hidden_up_link() {
            validate_native_type(item)?;
        }
    }
    write_native_type_record(context, "Tuple", &[("items", context.argument(0)?)])
}

fn native_function_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let parameters_value = context.value(context.argument(0)?)?;
    if parameters_value.kind() != ValueKind::Array {
        return Err(NativeError::new("Func expects an Array of parameter Types"));
    }
    for index in 0..parameters_value.sequence_len().expect("Array has a length") {
        let parameter = parameters_value
            .sequence_get(index)
            .expect("valid Array index");
        if !parameter.is_hidden_up_link() {
            validate_native_type(parameter)?;
        }
    }
    let result = context.argument(1)?;
    let result_value = context.value(result)?;
    if !result_value.is_hidden_up_link() {
        validate_native_type(result_value)?;
    }
    write_native_type_record(
        context,
        "Func",
        &[("parameters", context.argument(0)?), ("result", result)],
    )
}

fn write_native_type_record(
    context: &mut CallContext<'_, '_>,
    kind_name: &str,
    preserved_fields: &[(&str, RegisterId)],
) -> Result<(), NativeError> {
    let kind = context.scratch()?;
    context.set_atom(kind, kind_name)?;
    let mut fields = Vec::with_capacity(preserved_fields.len() + 1);
    fields.push(("kind".to_owned(), kind));
    fields.extend(
        preserved_fields
            .iter()
            .map(|(name, register)| ((*name).to_owned(), *register)),
    );
    context.make_dict(context.result(), &fields)
}

pub(crate) fn native_validate(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let type_register = context.argument(0)?;
    let value_register = context.argument(1)?;
    let descriptor = decode_native_type(context.value(type_register)?)?;
    let tag = context.scratch()?;
    let payload = context.scratch()?;
    match validate_value_ref(&descriptor, context.value(value_register)?, "value") {
        Ok(()) => {
            context.set_atom(tag, "Ok")?;
            if matches!(descriptor, TypeDescriptor::Declared(_))
                && context
                    .value(value_register)?
                    .declared_value_parts()
                    .is_none()
            {
                context.make_declared_value(payload, type_register, value_register)?;
            } else {
                context.copy(payload, value_register)?;
            }
        }
        Err(message) => {
            context.set_atom(tag, "Err")?;
            let error_message = context.scratch()?;
            context.set_string(error_message, message)?;
            context.make_dict(
                payload,
                &[
                    ("message".into(), error_message),
                    ("data".into(), value_register),
                    ("rule".into(), type_register),
                ],
            )?;
        }
    }
    context.make_tagged(context.result(), tag, payload)
}

pub(crate) fn decode_native_type(value: ValueRef<'_>) -> Result<TypeDescriptor, NativeError> {
    decode_type_ref_with(value, "Type", false).map_err(NativeError::new)
}

fn validate_native_type(value: ValueRef<'_>) -> Result<(), NativeError> {
    TypeGraph::default()
        .decode_persistent(value, "Type", &mut HashMap::new())
        .map(|_| ())
        .map_err(NativeError::new)
}

fn decode_type_ref(value: ValueRef<'_>, path: &str) -> Result<TypeDescriptor, String> {
    decode_type_ref_with(value, path, false)
}

fn decode_type_ref_with(
    value: ValueRef<'_>,
    path: &str,
    shallow_declared_types: bool,
) -> Result<TypeDescriptor, String> {
    let value = value.resolve_hidden_up_link()?;
    if let Some(native_type) = value.as_native_type() {
        return Ok(TypeDescriptor::Opaque(native_type.clone()));
    }
    if let Some((id, name, body)) = value.declared_type_parts() {
        return Ok(TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: id.clone(),
            name: name.to_owned(),
            body: Arc::new(if shallow_declared_types {
                TypeDescriptor::Any
            } else {
                decode_type_ref_with(body, path, false)?
            }),
        }));
    }
    let fields = value
        .dict_fields()
        .ok_or_else(|| format!("{path} must be a Dict"))?;
    let kind = value
        .dict_get("kind")
        .and_then(ValueRef::as_atom)
        .ok_or_else(|| format!("{path}.kind must be an Atom"))?;
    if kind == "WithAttributes" {
        if fields != ["attributes", "inner", "kind"] {
            return Err(format!(
                "{path} WithAttributes wrapper must have exactly attributes, inner, and kind fields"
            ));
        }
        let attributes = value
            .dict_get("attributes")
            .expect("validated wrapper field");
        if attributes.kind() != ValueKind::Dict {
            return Err(format!("{path}.attributes must be a Dict"));
        }
        return decode_type_ref_with(
            value.dict_get("inner").expect("validated wrapper field"),
            path,
            shallow_declared_types,
        );
    }
    let require = |expected: &[&str]| -> Result<(), String> {
        if fields.iter().copied().eq(expected.iter().copied()) {
            Ok(())
        } else {
            Err(format!("{path} has invalid fields for {kind}"))
        }
    };
    Ok(match kind {
        "Bound" | "'Bound" => {
            require(&["kind", "parameter"])?;
            let parameter = value
                .dict_get("parameter")
                .and_then(ValueRef::as_int)
                .and_then(|parameter| u32::try_from(parameter).ok())
                .ok_or_else(|| format!("{path}.parameter must be a non-negative Int"))?;
            TypeDescriptor::Bound(TypeParameterId(parameter))
        }
        "Named" => {
            require(&["kind", "name"])?;
            let name = value
                .dict_get("name")
                .and_then(ValueRef::as_str)
                .ok_or_else(|| format!("{path}.name must be a String"))?;
            TypeDescriptor::Named(name.to_owned())
        }
        "Any" => {
            require(&["kind"])?;
            TypeDescriptor::Any
        }
        "Never" => {
            require(&["kind"])?;
            TypeDescriptor::Never
        }
        "Type" => {
            require(&["kind"])?;
            TypeDescriptor::Type
        }
        "Dyn" => {
            require(&["kind"])?;
            TypeDescriptor::Dyn
        }
        "TypeOf" => {
            require(&["instance", "kind"])?;
            let instance = value
                .dict_get("instance")
                .ok_or_else(|| format!("{path}.instance is missing"))?;
            TypeDescriptor::TypeOf(Box::new(decode_type_ref_with(
                instance,
                &format!("{path}.instance"),
                shallow_declared_types,
            )?))
        }
        "Int" => {
            require(&["kind"])?;
            TypeDescriptor::Int
        }
        "Float" => {
            require(&["kind"])?;
            TypeDescriptor::Float
        }
        "String" => {
            require(&["kind"])?;
            TypeDescriptor::String
        }
        "Bytes" => {
            require(&["kind"])?;
            TypeDescriptor::Bytes
        }
        "Atom" => {
            require(&["kind", "tag"])?;
            let tag = value
                .dict_get("tag")
                .and_then(ValueRef::as_atom)
                .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
            TypeDescriptor::Atom(atom_from_name(tag))
        }
        "Array" => {
            require(&["item", "kind"])?;
            let item = value
                .dict_get("item")
                .ok_or_else(|| format!("{path}.item is missing"))?;
            TypeDescriptor::Array(Box::new(decode_type_ref_with(
                item,
                &format!("{path}.item"),
                shallow_declared_types,
            )?))
        }
        "Dict" => {
            require(&["item", "kind"])?;
            let item = value
                .dict_get("item")
                .ok_or_else(|| format!("{path}.item is missing"))?;
            TypeDescriptor::Dict(Box::new(decode_type_ref_with(
                item,
                &format!("{path}.item"),
                shallow_declared_types,
            )?))
        }
        "Tagged" => {
            require(&["kind", "payload", "tag"])?;
            let tag = value
                .dict_get("tag")
                .and_then(ValueRef::as_atom)
                .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
            let payload = value
                .dict_get("payload")
                .ok_or_else(|| format!("{path}.payload is missing"))?;
            TypeDescriptor::Tagged {
                tag: atom_from_name(tag),
                payload: Box::new(decode_type_ref_with(
                    payload,
                    &format!("{path}.payload"),
                    shallow_declared_types,
                )?),
            }
        }
        "Tuple" | "Union" => {
            let field = if kind == "Tuple" { "items" } else { "variants" };
            if kind == "Tuple" {
                require(&["items", "kind"])?;
            } else {
                require(&["kind", "variants"])?;
            }
            let sequence = value
                .dict_get(field)
                .ok_or_else(|| format!("{path}.{field} is missing"))?;
            if sequence.kind() != ValueKind::Array {
                return Err(format!("{path}.{field} must be an Array"));
            }
            let values = (0..sequence.sequence_len().expect("Array has a length"))
                .map(|index| {
                    decode_type_ref_with(
                        sequence.sequence_get(index).expect("valid Array index"),
                        &format!("{path}.{field}[{index}]"),
                        shallow_declared_types,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            if kind == "Union" && values.is_empty() {
                return Err(format!("{path}.variants must not be empty"));
            }
            if kind == "Tuple" {
                TypeDescriptor::Tuple(values)
            } else {
                TypeDescriptor::Union(values)
            }
        }
        "Struct" => {
            require(&["fields", "kind"])?;
            let fields_value = value
                .dict_get("fields")
                .ok_or_else(|| format!("{path}.fields is missing"))?;
            let names = fields_value
                .dict_fields()
                .ok_or_else(|| format!("{path}.fields must be a Dict"))?;
            TypeDescriptor::Struct(
                names
                    .iter()
                    .map(|name| {
                        let field = fields_value.dict_get(name).expect("Dict field exists");
                        Ok((
                            (*name).to_owned(),
                            decode_type_ref_with(
                                field,
                                &format!("{path}.fields.{name}"),
                                shallow_declared_types,
                            )?,
                        ))
                    })
                    .collect::<Result<_, String>>()?,
            )
        }
        "Enum" => {
            require(&["kind", "variants"])?;
            let variants = value
                .dict_get("variants")
                .ok_or_else(|| format!("{path}.variants is missing"))?;
            let names = variants
                .dict_fields()
                .ok_or_else(|| format!("{path}.variants must be a Dict"))?;
            if names.is_empty() {
                return Err(format!("{path}.variants must not be empty"));
            }
            TypeDescriptor::Enum(
                names
                    .iter()
                    .map(|name| {
                        let variant = variants.dict_get(name).expect("Dict field exists");
                        let variant_path = format!("{path}.variants.{name}");
                        let inner = strip_attributes_ref(variant, &variant_path)?;
                        let payload = if inner.as_atom() == Some("None") {
                            None
                        } else {
                            Some(Box::new(decode_type_ref_with(
                                inner,
                                &variant_path,
                                shallow_declared_types,
                            )?))
                        };
                        Ok(((*name).to_owned(), payload))
                    })
                    .collect::<Result<_, String>>()?,
            )
        }
        "Func" => {
            require(&["kind", "parameters", "result"])?;
            let parameters = value
                .dict_get("parameters")
                .ok_or_else(|| format!("{path}.parameters is missing"))?;
            if parameters.kind() != ValueKind::Array {
                return Err(format!("{path}.parameters must be an Array"));
            }
            let parameters = (0..parameters.sequence_len().expect("Array has a length"))
                .map(|index| {
                    decode_type_ref_with(
                        parameters.sequence_get(index).expect("valid Array index"),
                        &format!("{path}.parameters[{index}]"),
                        shallow_declared_types,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let result = value
                .dict_get("result")
                .ok_or_else(|| format!("{path}.result is missing"))?;
            TypeDescriptor::Function {
                parameters,
                result: Box::new(decode_type_ref_with(
                    result,
                    &format!("{path}.result"),
                    shallow_declared_types,
                )?),
            }
        }
        _ => return Err(format!("{path}.kind has unknown value '{kind}")),
    })
}

fn strip_attributes_ref<'a>(mut value: ValueRef<'a>, path: &str) -> Result<ValueRef<'a>, String> {
    loop {
        let Some(fields) = value.dict_fields() else {
            return Ok(value);
        };
        if value.dict_get("kind").and_then(ValueRef::as_atom) != Some("WithAttributes") {
            return Ok(value);
        }
        if fields != ["attributes", "inner", "kind"] {
            return Err(format!(
                "{path} WithAttributes wrapper must have exactly attributes, inner, and kind fields"
            ));
        }
        let attributes = value
            .dict_get("attributes")
            .expect("validated wrapper field");
        if attributes.kind() != ValueKind::Dict {
            return Err(format!("{path}.attributes must be a Dict"));
        }
        value = value.dict_get("inner").expect("validated wrapper field");
    }
}

fn validate_value_ref(
    descriptor: &TypeDescriptor,
    value: ValueRef<'_>,
    path: &str,
) -> Result<(), String> {
    match descriptor {
        TypeDescriptor::Declared(expected) => {
            if let Some((owner, _)) = value.declared_value_parts() {
                let Some((actual, _, _)) = owner.declared_type_parts() else {
                    return Err(format!("{path} has an invalid declared owner"));
                };
                if actual != &expected.id {
                    return Err(format!("{path} has a different declared type identity"));
                }
                Ok(())
            } else {
                validate_value_ref(&expected.body, value, path)
            }
        }
        TypeDescriptor::Any => Ok(()),
        TypeDescriptor::Never => Err(format!("{path} cannot have type Never")),
        TypeDescriptor::Type => decode_type_ref(value, path).map(|_| ()),
        TypeDescriptor::Dyn if value.kind() == ValueKind::Dyn => Ok(()),
        TypeDescriptor::TypeOf(expected) => {
            let actual = decode_type_ref(value, path)?;
            if assignable(&actual, expected) && assignable(expected, &actual) {
                Ok(())
            } else {
                Err(format!(
                    "{path} must describe {}, got {}",
                    expected.display_name(),
                    actual.display_name()
                ))
            }
        }
        TypeDescriptor::Int if value.kind() == ValueKind::Int => Ok(()),
        TypeDescriptor::Float if value.kind() == ValueKind::Float => Ok(()),
        TypeDescriptor::String if value.kind() == ValueKind::String => Ok(()),
        TypeDescriptor::Bytes if value.kind() == ValueKind::Bytes => Ok(()),
        TypeDescriptor::Opaque(expected) if value.kind() == ValueKind::Opaque => {
            let actual = value.opaque_native_type().expect("ValueKind checked");
            if actual == expected {
                Ok(())
            } else {
                Err(format!(
                    "{path} must be {}, got {}",
                    expected.qualified_name(),
                    actual.qualified_name()
                ))
            }
        }
        TypeDescriptor::Atom(expected) if value.as_atom() == Some(expected.name()) => Ok(()),
        TypeDescriptor::Atom(expected) => Err(format!("{path} must be '{}", expected.name())),
        TypeDescriptor::Array(item) => {
            if value.kind() != ValueKind::Array {
                return Err(format!("{path} must be an Array"));
            }
            for index in 0..value.sequence_len().expect("Array has a length") {
                validate_value_ref(
                    item,
                    value.sequence_get(index).expect("valid Array index"),
                    &format!("{path}[{index}]"),
                )?;
            }
            Ok(())
        }
        TypeDescriptor::Dict(item) => {
            let Some(names) = value.dict_fields() else {
                return Err(format!("{path} must be a Dict"));
            };
            for name in names {
                validate_value_ref(
                    item,
                    value.dict_get(name).expect("Dict field exists"),
                    &format!("{path}.{name}"),
                )?;
            }
            Ok(())
        }
        TypeDescriptor::Tagged { tag, payload } => {
            let Some((actual_tag, actual_payload)) = value.tagged_parts() else {
                return Err(format!("{path} must be a Tagged value"));
            };
            if actual_tag.as_atom() != Some(tag.name()) {
                return Err(format!("{path} must have tag '{}", tag.name()));
            }
            validate_value_ref(payload, actual_payload, &format!("{path}.payload"))
        }
        TypeDescriptor::Tuple(items) => {
            if value.kind() != ValueKind::Tuple {
                return Err(format!("{path} must be a Tuple"));
            }
            if value.sequence_len() != Some(items.len()) {
                return Err(format!("{path} must have {} tuple items", items.len()));
            }
            for (index, item) in items.iter().enumerate() {
                validate_value_ref(
                    item,
                    value.sequence_get(index).expect("valid Tuple index"),
                    &format!("{path}.{index}"),
                )?;
            }
            Ok(())
        }
        TypeDescriptor::Struct(items) => {
            let Some(names) = value.dict_fields() else {
                return Err(format!("{path} must be a Dict"));
            };
            if !items.keys().eq(names.iter()) {
                return Err(format!("{path} has a different field shape"));
            }
            for (name, item) in items {
                validate_value_ref(
                    item,
                    value.dict_get(name).expect("matching shape"),
                    &format!("{path}.{name}"),
                )?;
            }
            Ok(())
        }
        TypeDescriptor::Enum(variants) => {
            if let Some(tag) = value.as_atom() {
                return match variants.get(tag) {
                    Some(None) => Ok(()),
                    Some(Some(_)) => Err(format!("{path} variant '{tag} requires a payload")),
                    None => Err(format!("{path} has unknown Enum variant '{tag}")),
                };
            }
            let Some((tag_value, payload_value)) = value.tagged_parts() else {
                return Err(format!("{path} must be a unit Atom or a Tagged value"));
            };
            let tag = tag_value
                .as_atom()
                .ok_or_else(|| format!("{path} Tagged tag must be an Atom"))?;
            match variants.get(tag) {
                Some(Some(payload)) => {
                    validate_value_ref(payload, payload_value, &format!("{path}.{tag}"))
                }
                Some(None) => Err(format!("{path} variant '{tag} does not accept a payload")),
                None => Err(format!("{path} has unknown Enum variant '{tag}")),
            }
        }
        TypeDescriptor::Union(variants) => {
            if variants
                .iter()
                .any(|variant| validate_value_ref(variant, value, path).is_ok())
            {
                Ok(())
            } else {
                Err(format!("{path} does not match any Union variant"))
            }
        }
        TypeDescriptor::Function { parameters, .. }
            if value.function_arity() == Some(parameters.len()) =>
        {
            Ok(())
        }
        TypeDescriptor::Function { parameters, .. } if value.kind() == ValueKind::Func => {
            Err(format!("{path} must accept {} arguments", parameters.len()))
        }
        descriptor => Err(format!(
            "{path} must be {}, got {:?}",
            descriptor.display_name(),
            value.kind()
        )),
    }
}

fn kind_entry(kind: &str) -> (String, Value) {
    ("kind".into(), Value::atom(kind))
}

pub(crate) fn decode_type(value: &Value, path: &str) -> Result<TypeDescriptor, String> {
    if let Value::NativeType(native_type) = value {
        return Ok(TypeDescriptor::Opaque(native_type.clone()));
    }
    if let Value::DeclaredType(declared) = value {
        return Ok(TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: declared.id().clone(),
            name: declared.name().to_owned(),
            body: Arc::new(decode_type(declared.body(), path)?),
        }));
    }
    let Value::Dict(metadata) = value else {
        return Err(format!("{path} must be a Dict"));
    };
    let kind = metadata
        .get("kind")
        .ok_or_else(|| format!("{path}.kind is missing"))?;
    let Value::Atom(kind) = kind else {
        return Err(format!("{path}.kind must be an Atom"));
    };
    if kind.name() == "WithAttributes" {
        require_fields(metadata, path, &["attributes", "inner", "kind"])?;
        if !matches!(metadata.get("attributes"), Some(Value::Dict(_))) {
            return Err(format!("{path}.attributes must be a Dict"));
        }
        return decode_type(metadata.get("inner").expect("required field"), path);
    }
    let descriptor = match kind.name() {
        "Bound" | "'Bound" => {
            require_fields(metadata, path, &["kind", "parameter"])?;
            let Some(Value::Int(parameter)) = metadata.get("parameter") else {
                return Err(format!("{path}.parameter must be an Int"));
            };
            TypeDescriptor::Bound(TypeParameterId(
                u32::try_from(*parameter)
                    .map_err(|_| format!("{path}.parameter must be a non-negative Int"))?,
            ))
        }
        "Named" => {
            require_fields(metadata, path, &["kind", "name"])?;
            let Some(Value::String(name)) = metadata.get("name") else {
                return Err(format!("{path}.name must be a String"));
            };
            TypeDescriptor::Named(name.to_string())
        }
        "Any" => {
            require_fields(metadata, path, &["kind"])?;
            TypeDescriptor::Any
        }
        "Never" => {
            require_fields(metadata, path, &["kind"])?;
            TypeDescriptor::Never
        }
        "Type" => {
            require_fields(metadata, path, &["kind"])?;
            TypeDescriptor::Type
        }
        "Dyn" => {
            require_fields(metadata, path, &["kind"])?;
            TypeDescriptor::Dyn
        }
        "TypeOf" => {
            require_fields(metadata, path, &["instance", "kind"])?;
            TypeDescriptor::TypeOf(Box::new(decode_type(
                metadata.get("instance").expect("required field"),
                &format!("{path}.instance"),
            )?))
        }
        "Int" => {
            require_fields(metadata, path, &["kind"])?;
            TypeDescriptor::Int
        }
        "Float" => {
            require_fields(metadata, path, &["kind"])?;
            TypeDescriptor::Float
        }
        "String" => {
            require_fields(metadata, path, &["kind"])?;
            TypeDescriptor::String
        }
        "Bytes" => {
            require_fields(metadata, path, &["kind"])?;
            TypeDescriptor::Bytes
        }
        "Atom" => {
            require_fields(metadata, path, &["kind", "tag"])?;
            let Value::Atom(tag) = metadata.get("tag").expect("required field") else {
                return Err(format!("{path}.tag must be an Atom"));
            };
            TypeDescriptor::Atom(tag.clone())
        }
        "Array" => {
            require_fields(metadata, path, &["item", "kind"])?;
            TypeDescriptor::Array(Box::new(decode_type(
                metadata.get("item").expect("required field"),
                &format!("{path}.item"),
            )?))
        }
        "Dict" => {
            require_fields(metadata, path, &["item", "kind"])?;
            TypeDescriptor::Dict(Box::new(decode_type(
                metadata.get("item").expect("required field"),
                &format!("{path}.item"),
            )?))
        }
        "Tagged" => {
            require_fields(metadata, path, &["kind", "payload", "tag"])?;
            let Value::Atom(tag) = metadata.get("tag").expect("required field") else {
                return Err(format!("{path}.tag must be an Atom"));
            };
            TypeDescriptor::Tagged {
                tag: tag.clone(),
                payload: Box::new(decode_type(
                    metadata.get("payload").expect("required field"),
                    &format!("{path}.payload"),
                )?),
            }
        }
        "Tuple" => {
            require_fields(metadata, path, &["items", "kind"])?;
            let Value::Array(items) = metadata.get("items").expect("required field") else {
                return Err(format!("{path}.items must be an Array"));
            };
            TypeDescriptor::Tuple(
                items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| decode_type(item, &format!("{path}.items[{index}]")))
                    .collect::<Result<_, _>>()?,
            )
        }
        "Struct" => {
            require_fields(metadata, path, &["fields", "kind"])?;
            let Value::Dict(fields) = metadata.get("fields").expect("required field") else {
                return Err(format!("{path}.fields must be a Dict"));
            };
            let fields = fields
                .shape()
                .fields()
                .iter()
                .zip(fields.values())
                .map(|(name, field)| {
                    Ok((
                        name.clone(),
                        decode_type(field, &format!("{path}.fields.{name}"))?,
                    ))
                })
                .collect::<Result<_, String>>()?;
            TypeDescriptor::Struct(fields)
        }
        "Enum" => {
            require_fields(metadata, path, &["kind", "variants"])?;
            let Value::Dict(variants) = metadata.get("variants").expect("required field") else {
                return Err(format!("{path}.variants must be a Dict"));
            };
            if variants.values().is_empty() {
                return Err(format!("{path}.variants must not be empty"));
            }
            TypeDescriptor::Enum(
                variants
                    .shape()
                    .fields()
                    .iter()
                    .zip(variants.values())
                    .map(|(name, variant)| {
                        let variant_path = format!("{path}.variants.{name}");
                        let inner = strip_attributes_value(variant, &variant_path)?;
                        let payload = if matches!(inner, Value::Atom(atom) if atom.name() == "None")
                        {
                            None
                        } else {
                            Some(Box::new(decode_type(inner, &variant_path)?))
                        };
                        Ok((name.clone(), payload))
                    })
                    .collect::<Result<_, String>>()?,
            )
        }
        "Union" => {
            require_fields(metadata, path, &["kind", "variants"])?;
            let Value::Array(variants) = metadata.get("variants").expect("required field") else {
                return Err(format!("{path}.variants must be an Array"));
            };
            if variants.is_empty() {
                return Err(format!("{path}.variants must not be empty"));
            }
            TypeDescriptor::Union(
                variants
                    .iter()
                    .enumerate()
                    .map(|(index, variant)| {
                        decode_type(variant, &format!("{path}.variants[{index}]"))
                    })
                    .collect::<Result<_, _>>()?,
            )
        }
        "Func" => {
            require_fields(metadata, path, &["kind", "parameters", "result"])?;
            let Value::Array(parameters) = metadata.get("parameters").expect("required field")
            else {
                return Err(format!("{path}.parameters must be an Array"));
            };
            TypeDescriptor::Function {
                parameters: parameters
                    .iter()
                    .enumerate()
                    .map(|(index, parameter)| {
                        decode_type(parameter, &format!("{path}.parameters[{index}]"))
                    })
                    .collect::<Result<_, _>>()?,
                result: Box::new(decode_type(
                    metadata.get("result").expect("required field"),
                    &format!("{path}.result"),
                )?),
            }
        }
        other => return Err(format!("{path}.kind has unknown value '{other}")),
    };
    Ok(descriptor)
}

fn strip_attributes_value<'a>(mut value: &'a Value, path: &str) -> Result<&'a Value, String> {
    loop {
        let Value::Dict(metadata) = value else {
            return Ok(value);
        };
        if !matches!(metadata.get("kind"), Some(Value::Atom(kind)) if kind.name() == "WithAttributes")
        {
            return Ok(value);
        }
        require_fields(metadata, path, &["attributes", "inner", "kind"])?;
        if !matches!(metadata.get("attributes"), Some(Value::Dict(_))) {
            return Err(format!("{path}.attributes must be a Dict"));
        }
        value = metadata.get("inner").expect("required field");
    }
}

fn require_fields(metadata: &crate::Dict, path: &str, fields: &[&str]) -> Result<(), String> {
    let actual = metadata
        .shape()
        .fields()
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    if actual != fields {
        return Err(format!("{path} has fields {actual:?}, expected {fields:?}"));
    }
    Ok(())
}

fn infer_expr(expression: &Expr, environment: &HashMap<String, TypeDescriptor>) -> TypeDescriptor {
    infer_expr_with(expression, environment, &mut |_, _| {})
}

struct GenericInference<'a> {
    schemes: HashMap<String, TypeScheme>,
    scheme_scopes: Vec<HashMap<String, Option<TypeScheme>>>,
    top_level_inferred_schemes: HashMap<String, TypeScheme>,
    inferred_schemes: HashMap<crate::Location, TypeScheme>,
    placeholder_obligations: Vec<(InferenceVariableId, crate::Location, String)>,
    hir: &'a HirProgram,
    external_interfaces: &'a BTreeMap<String, ModuleInterface>,
    named_types: &'a BTreeMap<String, TypeDescriptor>,
    local_annotations: &'a HashMap<crate::Location, TypeDescriptor>,
    builtin_tuple_available: bool,
    query: Option<crate::query::QueryContext>,
    next_variable: u32,
    closure_inference_depth: usize,
    delayed_initializer_depth: usize,
    recursive_body_inference_depth: usize,
    numeric_variables: HashSet<InferenceVariableId>,
    not_variables: HashSet<InferenceVariableId>,
    ordered_variables: HashSet<InferenceVariableId>,
    field_requirements: HashMap<InferenceVariableId, BTreeMap<String, TypeDescriptor>>,
    recursive_equations: HashMap<InferenceVariableId, TypeDescriptor>,
    substitutions: HashMap<InferenceVariableId, TypeDescriptor>,
    records: HashMap<crate::Location, TypeDescriptor>,
    pattern_diagnostics: BTreeMap<crate::Location, String>,
    pattern_binding_types: HashMap<crate::Location, TypeDescriptor>,
    propagation_boundaries: Vec<Option<PropagationRequirement>>,
    return_boundaries: Vec<Option<ReturnBoundary>>,
    propagation_families: HashMap<crate::Location, PropagationFamily>,
    not_families: HashMap<crate::Location, NotFamily>,
    failure_location: Option<crate::Location>,
    checking_named_pairs: HashSet<(String, String)>,
}

#[derive(Clone)]
enum PropagationRequirement {
    Option,
    Result(Vec<TypeDescriptor>),
}

struct ReturnBoundary {
    expected: Option<TypeDescriptor>,
    values: Vec<TypeDescriptor>,
}

#[derive(Default)]
struct DefinitionComponentPlan {
    recursive: HashSet<crate::Location>,
    indirect_recursive: HashSet<crate::Location>,
    acyclic: Vec<crate::Location>,
}

fn definition_component_plan(block: &Block, hir: &HirProgram) -> DefinitionComponentPlan {
    let candidates = block
        .value
        .bindings
        .iter()
        .filter(|binding| {
            binding.value.kind == BindingKind::Def
                && binding.value.annotation.is_none()
                && binding.value.type_parameters.is_empty()
                && matches!(binding.value.value.value, ExprKind::Closure { .. })
                && !block.value.bindings.iter().any(|candidate| {
                    candidate.value.kind == BindingKind::Decl
                        && candidate.value.name.value == binding.value.name.value
                })
        })
        .filter_map(|binding| {
            hir.definitions()
                .iter()
                .find(|definition| {
                    definition.kind == HirDefinitionKind::DefinitionSlot
                        && definition.name == binding.value.name.value
                        && definition.value.is_some_and(|value| {
                            hir.expression(value).is_some_and(|expression| {
                                expression.location == binding.value.value.location
                            })
                        })
                })
                .map(|definition| (definition.id, binding.value.name.location))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return DefinitionComponentPlan::default();
    }
    let indices = candidates
        .iter()
        .enumerate()
        .map(|(index, (definition, _))| (*definition, index))
        .collect::<HashMap<_, _>>();
    let direct_dependencies = hir
        .definitions()
        .iter()
        .filter_map(|definition| {
            let root = definition.value?;
            let mut dependencies = Vec::new();
            for expression in hir.expressions() {
                let Some(reference) = expression.reference.and_then(|id| hir.reference(id)) else {
                    continue;
                };
                let HirResolution::Definition(target) = reference.resolution else {
                    continue;
                };
                let mut owner = Some(expression.id);
                while let Some(current) = owner {
                    if current == root {
                        if !dependencies.contains(&target) {
                            dependencies.push(target);
                        }
                        break;
                    }
                    owner = hir
                        .expression(current)
                        .and_then(|expression| expression.parent);
                }
            }
            dependencies.sort_unstable();
            Some((definition.id, dependencies))
        })
        .collect::<HashMap<_, _>>();
    let mut edges = vec![Vec::new(); candidates.len()];
    let mut indirect_edge_sources = HashSet::new();
    for (index, (definition, _)) in candidates.iter().enumerate() {
        let mut pending = direct_dependencies
            .get(definition)
            .into_iter()
            .flatten()
            .map(|dependency| (*dependency, false))
            .collect::<Vec<_>>();
        let mut visited = HashSet::new();
        while let Some((target, indirect)) = pending.pop() {
            if !visited.insert((target, indirect)) {
                continue;
            }
            if let Some(&target) = indices.get(&target) {
                if !edges[index].contains(&target) {
                    edges[index].push(target);
                }
                if indirect {
                    indirect_edge_sources.insert(index);
                }
                continue;
            }
            if let Some(dependencies) = direct_dependencies.get(&target) {
                pending.extend(dependencies.iter().map(|dependency| (*dependency, true)));
            }
        }
        edges[index].sort_unstable();
    }

    fn reaches(
        current: usize,
        target: usize,
        edges: &[Vec<usize>],
        visited: &mut HashSet<usize>,
    ) -> bool {
        visited.insert(current)
            && edges[current]
                .iter()
                .any(|next| *next == target || reaches(*next, target, edges, visited))
    }

    let recursive_indices = (0..candidates.len())
        .filter(|index| reaches(*index, *index, &edges, &mut HashSet::new()))
        .collect::<HashSet<_>>();
    let mut ordered = Vec::new();
    let mut visited = HashSet::new();
    fn visit(
        node: usize,
        edges: &[Vec<usize>],
        recursive: &HashSet<usize>,
        visited: &mut HashSet<usize>,
        ordered: &mut Vec<usize>,
    ) {
        if recursive.contains(&node) || !visited.insert(node) {
            return;
        }
        for dependency in &edges[node] {
            visit(*dependency, edges, recursive, visited, ordered);
        }
        ordered.push(node);
    }
    for node in 0..candidates.len() {
        visit(node, &edges, &recursive_indices, &mut visited, &mut ordered);
    }
    DefinitionComponentPlan {
        recursive: recursive_indices
            .iter()
            .map(|index| candidates[*index].1)
            .collect(),
        indirect_recursive: recursive_indices
            .intersection(&indirect_edge_sources)
            .map(|index| candidates[*index].1)
            .collect(),
        acyclic: ordered
            .into_iter()
            .map(|index| candidates[index].1)
            .collect(),
    }
}

impl<'a> GenericInference<'a> {
    fn new(
        schemes: &HashMap<String, TypeScheme>,
        hir: &'a HirProgram,
        external_interfaces: &'a BTreeMap<String, ModuleInterface>,
        named_types: &'a BTreeMap<String, TypeDescriptor>,
        local_annotations: &'a HashMap<crate::Location, TypeDescriptor>,
        builtin_tuple_available: bool,
        query: Option<crate::query::QueryContext>,
    ) -> Self {
        Self {
            schemes: schemes.clone(),
            scheme_scopes: vec![HashMap::new()],
            top_level_inferred_schemes: HashMap::new(),
            inferred_schemes: HashMap::new(),
            placeholder_obligations: Vec::new(),
            hir,
            external_interfaces,
            named_types,
            local_annotations,
            builtin_tuple_available,
            query,
            next_variable: 0,
            closure_inference_depth: 0,
            delayed_initializer_depth: 0,
            recursive_body_inference_depth: 0,
            numeric_variables: HashSet::new(),
            not_variables: HashSet::new(),
            ordered_variables: HashSet::new(),
            field_requirements: HashMap::new(),
            recursive_equations: HashMap::new(),
            substitutions: HashMap::new(),
            records: HashMap::new(),
            pattern_diagnostics: BTreeMap::new(),
            pattern_binding_types: HashMap::new(),
            propagation_boundaries: vec![None],
            return_boundaries: vec![None],
            propagation_families: HashMap::new(),
            not_families: HashMap::new(),
            failure_location: None,
            checking_named_pairs: HashSet::new(),
        }
    }

    fn take_failure_location(&mut self, fallback: crate::Location) -> crate::Location {
        self.failure_location.take().unwrap_or(fallback)
    }

    fn expose_named(&self, ty: &TypeDescriptor) -> TypeDescriptor {
        let mut current = self.resolve(ty);
        let mut visited = HashSet::new();
        while let TypeDescriptor::Named(name) = &current {
            if !visited.insert(name.clone()) {
                break;
            }
            let Some(target) = self.named_type(name) else {
                break;
            };
            current = self.resolve(target);
        }
        current
    }

    fn named_type(&self, name: &str) -> Option<&TypeDescriptor> {
        self.named_types.get(name).or_else(|| {
            let short = display_named_type(name);
            let mut candidates = self
                .named_types
                .iter()
                .filter(|(candidate, _)| display_named_type(candidate) == short)
                .map(|(_, descriptor)| descriptor);
            let candidate = candidates.next()?;
            let normalized = normalize_named_names(candidate);
            candidates
                .all(|other| normalize_named_names(other) == normalized)
                .then_some(candidate)
        })
    }

    fn named_identity(&self, ty: &TypeDescriptor) -> Option<String> {
        match ty {
            TypeDescriptor::Named(name) => Some(name.clone()),
            _ => None,
        }
    }

    fn declared_identity(&self, ty: &TypeDescriptor) -> Option<crate::value::DeclaredTypeId> {
        fn find(
            inference: &GenericInference<'_>,
            ty: &TypeDescriptor,
            named: &mut HashSet<String>,
            variables: &mut HashSet<InferenceVariableId>,
        ) -> Option<crate::value::DeclaredTypeId> {
            match ty {
                TypeDescriptor::Declared(declared) => Some(declared.id.clone()),
                TypeDescriptor::Named(name) if named.insert(name.clone()) => inference
                    .named_type(name)
                    .and_then(|ty| find(inference, ty, named, variables)),
                TypeDescriptor::Inference(variable) if variables.insert(*variable) => inference
                    .substitutions
                    .get(variable)
                    .and_then(|ty| find(inference, ty, named, variables)),
                _ => None,
            }
        }

        find(self, ty, &mut HashSet::new(), &mut HashSet::new())
    }

    fn finish_return_boundary(
        &mut self,
        tail: TypeDescriptor,
        boundary: ReturnBoundary,
    ) -> Result<TypeDescriptor, String> {
        if let Some(expected) = boundary.expected {
            for value in &boundary.values {
                self.check(value, &expected)?;
            }
            self.check(&tail, &expected)?;
            return Ok(expected);
        }
        let mut values = boundary.values;
        values.push(tail);
        Ok(common_type(values).unwrap_or(TypeDescriptor::Never))
    }

    fn record_propagation(&mut self, requirement: PropagationRequirement) -> Result<(), String> {
        let boundary = self
            .propagation_boundaries
            .last_mut()
            .expect("module propagation boundary exists");
        match (boundary.as_mut(), requirement) {
            (None, requirement) => *boundary = Some(requirement),
            (Some(PropagationRequirement::Option), PropagationRequirement::Option) => {}
            (
                Some(PropagationRequirement::Result(errors)),
                PropagationRequirement::Result(mut more),
            ) => {
                errors.append(&mut more);
            }
            _ => return Err("cannot mix Option and Result propagation in one boundary".into()),
        }
        Ok(())
    }

    fn finish_propagation_boundary(
        &mut self,
        result: TypeDescriptor,
        expected: Option<&TypeDescriptor>,
        requirement: Option<PropagationRequirement>,
    ) -> Result<TypeDescriptor, String> {
        let Some(requirement) = requirement else {
            return Ok(result);
        };
        let resolved = self.resolve(&result);
        match requirement {
            PropagationRequirement::Option => match resolved {
                TypeDescriptor::Enum(ref variants) if option_parts(variants).is_some() => {
                    Ok(resolved)
                }
                TypeDescriptor::Tagged { tag, payload } if tag.name() == "Some" => {
                    Ok(option_descriptor(*payload))
                }
                TypeDescriptor::Atom(tag) if tag.name() == "None" => {
                    match expected.map(|ty| self.resolve(ty)) {
                        Some(ref expected @ TypeDescriptor::Enum(ref variants)) if option_parts(variants).is_some() => Ok(expected.clone()),
                        _ => Err("Option propagation boundary ending in 'None needs an expected Option success type".into()),
                    }
                }
                _ => Err(format!(
                    "Option propagation requires an Option-shaped boundary result, found {}",
                    resolved.display_name()
                )),
            },
            PropagationRequirement::Result(errors) => {
                let expected = expected.map(|ty| self.resolve(ty));
                let boundary_error = expected
                    .as_ref()
                    .and_then(result_parts)
                    .map(|(_, err)| err.clone())
                    .or_else(|| common_type(errors.clone()))
                    .ok_or_else(|| "cannot infer Result propagation error type".to_owned())?;
                for error in &errors {
                    self.check(error, &boundary_error)?;
                }
                match resolved {
                    TypeDescriptor::Enum(ref variants) if result_parts(&TypeDescriptor::Enum(variants.clone())).is_some() => {
                        let (_, result_error) = result_parts(&resolved).expect("checked Result shape");
                        for error in &errors { self.check(error, result_error)?; }
                        Ok(resolved)
                    }
                    TypeDescriptor::Tagged { tag, payload } if tag.name() == "Ok" => {
                        Ok(result_descriptor(*payload, boundary_error))
                    }
                    TypeDescriptor::Tagged { tag, payload } if tag.name() == "Err" => {
                        match expected {
                            Some(ref expected @ TypeDescriptor::Enum(ref variants)) if result_parts(&TypeDescriptor::Enum(variants.clone())).is_some() => {
                                self.check(&payload, &boundary_error)?;
                                Ok(expected.clone())
                            }
                            _ => Err("Result propagation boundary ending in 'Err(_) needs an expected Result success type".into()),
                        }
                    }
                    _ => Err(format!("Result propagation requires a Result-shaped boundary result, found {}", resolved.display_name())),
                }
            }
        }
    }

    fn instantiate(&mut self, scheme: &TypeScheme) -> TypeDescriptor {
        let mut implicit_parameters = Vec::new();
        if scheme.parameters.is_empty() {
            collect_bound_parameters(&scheme.body, &mut implicit_parameters);
        }
        let parameters = scheme
            .parameters
            .iter()
            .map(|parameter| parameter.id)
            .chain(implicit_parameters);
        let mut variables = parameters
            .map(|parameter| {
                let variable = InferenceVariableId(self.next_variable);
                self.next_variable += 1;
                (parameter, variable)
            })
            .collect();
        self.instantiate_with(&scheme.body, &mut variables)
    }

    fn scoped_scheme(&self, name: &str) -> Option<Option<TypeScheme>> {
        self.scheme_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn scheme(&self, name: &str) -> Option<TypeScheme> {
        match self.scoped_scheme(name) {
            Some(scheme) => scheme,
            None => self.schemes.get(name).cloned(),
        }
    }

    fn set_local_scheme(&mut self, name: String, scheme: Option<TypeScheme>) {
        self.scheme_scopes
            .last_mut()
            .expect("type inference always has a scheme scope")
            .insert(name, scheme);
    }

    fn explicit_scheme(&self, callee: &Expr) -> Option<TypeScheme> {
        match &callee.value {
            ExprKind::Variable(name) => self.scheme(&name.value),
            ExprKind::Field { receiver, field } => match &receiver.value {
                ExprKind::Variable(module) => self
                    .external_interfaces
                    .get(&module.value)
                    .and_then(|interface| interface.exports.get(&field.value))
                    .cloned(),
                _ => None,
            },
            _ => None,
        }
    }

    fn fresh_variable(&mut self) -> TypeDescriptor {
        let variable = InferenceVariableId(self.next_variable);
        self.next_variable += 1;
        TypeDescriptor::Inference(variable)
    }

    fn freshen_join_context(
        &mut self,
        expected: &TypeDescriptor,
        environment: &HashMap<String, TypeDescriptor>,
    ) -> (
        TypeDescriptor,
        HashMap<String, TypeDescriptor>,
        HashMap<InferenceVariableId, InferenceVariableId>,
    ) {
        let expected = self.resolve(expected);
        let mut variables = Vec::new();
        collect_inference_variables(&expected, &mut variables);
        let replacements = variables
            .into_iter()
            .map(|variable| {
                let TypeDescriptor::Inference(fresh) = self.fresh_variable() else {
                    unreachable!("fresh variable descriptor")
                };
                if self.numeric_variables.contains(&variable) {
                    self.numeric_variables.insert(fresh);
                }
                if self.not_variables.contains(&variable) {
                    self.not_variables.insert(fresh);
                }
                if self.ordered_variables.contains(&variable) {
                    self.ordered_variables.insert(fresh);
                }
                (variable, fresh)
            })
            .collect::<HashMap<_, _>>();
        let expected = replace_inference_variables(&expected, &replacements);
        let environment = environment
            .iter()
            .map(|(name, descriptor)| {
                (
                    name.clone(),
                    replace_inference_variables(&self.resolve(descriptor), &replacements),
                )
            })
            .collect();
        (expected, environment, replacements)
    }

    fn merge_join_evidence(
        &mut self,
        branches: &[HashMap<InferenceVariableId, InferenceVariableId>],
    ) -> Result<(), String> {
        let Some(first) = branches.first() else {
            return Ok(());
        };
        let originals = first.keys().copied().collect::<Vec<_>>();
        for original in originals {
            let mut evidence = Vec::new();
            for branch in branches {
                let Some(fresh) = branch.get(&original) else {
                    continue;
                };
                let resolved = self.resolve(&TypeDescriptor::Inference(*fresh));
                if !contains_type_variable(&resolved) {
                    evidence.push(resolved);
                }
            }
            if !evidence.is_empty() {
                let joined = join_all_types(evidence);
                self.check(&joined, &TypeDescriptor::Inference(original))?;
                let merged = self.resolve(&TypeDescriptor::Inference(original));
                for branch in branches {
                    let Some(fresh) = branch.get(&original) else {
                        continue;
                    };
                    if contains_type_variable(&self.resolve(&TypeDescriptor::Inference(*fresh))) {
                        self.check(&merged, &TypeDescriptor::Inference(*fresh))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn merge_structural_join_evidence(
        &mut self,
        branches: &[TypeDescriptor],
    ) -> Result<(), String> {
        fn collect(
            unresolved: &TypeDescriptor,
            evidence: &TypeDescriptor,
            collected: &mut HashMap<InferenceVariableId, Vec<TypeDescriptor>>,
            collection_element: bool,
        ) {
            if let TypeDescriptor::Inference(variable) = unresolved {
                if collection_element && !contains_type_variable(evidence) {
                    collected
                        .entry(*variable)
                        .or_default()
                        .push(evidence.clone());
                }
                return;
            }
            match (unresolved, evidence) {
                (TypeDescriptor::Array(left), TypeDescriptor::Array(right))
                | (TypeDescriptor::Dict(left), TypeDescriptor::Dict(right)) => {
                    collect(left, right, collected, true);
                }
                (TypeDescriptor::TypeOf(left), TypeDescriptor::TypeOf(right)) => {
                    collect(left, right, collected, collection_element);
                }
                (
                    TypeDescriptor::Tagged {
                        tag: left_tag,
                        payload: left,
                    },
                    TypeDescriptor::Tagged {
                        tag: right_tag,
                        payload: right,
                    },
                ) if left_tag == right_tag => {
                    collect(left, right, collected, collection_element);
                }
                (TypeDescriptor::Tuple(left), TypeDescriptor::Tuple(right))
                    if left.len() == right.len() =>
                {
                    for (left, right) in left.iter().zip(right) {
                        collect(left, right, collected, collection_element);
                    }
                }
                (TypeDescriptor::Struct(left), TypeDescriptor::Struct(right))
                    if left.keys().eq(right.keys()) =>
                {
                    for (name, left) in left {
                        collect(left, &right[name], collected, collection_element);
                    }
                }
                (TypeDescriptor::Enum(left), TypeDescriptor::Enum(right))
                    if left.keys().eq(right.keys()) =>
                {
                    for (name, left) in left {
                        if let (Some(left), Some(right)) = (left.as_deref(), right[name].as_deref())
                        {
                            collect(left, right, collected, collection_element);
                        }
                    }
                }
                (
                    TypeDescriptor::Function {
                        parameters: left_parameters,
                        result: left_result,
                    },
                    TypeDescriptor::Function {
                        parameters: right_parameters,
                        result: right_result,
                    },
                ) if left_parameters.len() == right_parameters.len() => {
                    for (left, right) in left_parameters.iter().zip(right_parameters) {
                        collect(left, right, collected, collection_element);
                    }
                    collect(left_result, right_result, collected, collection_element);
                }
                _ => {}
            }
        }

        let resolved = branches
            .iter()
            .map(|branch| self.resolve(branch))
            .collect::<Vec<_>>();
        let mut collected = HashMap::new();
        for (index, branch) in resolved.iter().enumerate() {
            for evidence in resolved.iter().skip(index + 1) {
                collect(branch, evidence, &mut collected, false);
                collect(evidence, branch, &mut collected, false);
            }
        }
        for (variable, evidence) in collected {
            self.check(
                &join_all_types(evidence),
                &TypeDescriptor::Inference(variable),
            )?;
        }
        Ok(())
    }

    fn generalize_local_closure(
        &mut self,
        descriptor: &TypeDescriptor,
        first_owned_variable: u32,
        location: crate::Location,
    ) -> Result<Option<TypeScheme>, String> {
        let descriptor = self.resolve(descriptor);
        let mut variables = Vec::new();
        collect_inference_variables(&descriptor, &mut variables);
        variables.retain(|variable| variable.0 >= first_owned_variable);
        variables.dedup();
        if variables
            .iter()
            .any(|variable| self.field_requirements.contains_key(variable))
        {
            return Ok(None);
        }
        if variables.is_empty()
            || variables.iter().any(|variable| {
                self.numeric_variables.contains(variable)
                    || self.not_variables.contains(variable)
                    || self.ordered_variables.contains(variable)
            })
        {
            return Ok(None);
        }
        let mut bound_parameters = Vec::new();
        collect_bound_parameters(&descriptor, &mut bound_parameters);
        let first_parameter = bound_parameters
            .iter()
            .map(|parameter| parameter.0)
            .max()
            .map_or(Some(0), |parameter| parameter.checked_add(1))
            .ok_or_else(|| "inferred type parameter identity overflow".to_owned())?;
        let replacements = variables
            .iter()
            .enumerate()
            .map(|(index, variable)| (*variable, TypeParameterId(first_parameter + index as u32)))
            .collect::<HashMap<_, _>>();
        let parameters = variables
            .iter()
            .enumerate()
            .map(|(index, _)| TypeParameter {
                id: TypeParameterId(first_parameter + index as u32),
                name: inferred_type_parameter_name(index),
                location,
            })
            .collect();
        Ok(Some(TypeScheme {
            parameters,
            body: bind_inference_variables(&descriptor, &replacements),
        }))
    }

    fn unresolved_placeholder_since(&self, start: usize) -> Option<(crate::Location, String)> {
        self.placeholder_obligations[start..]
            .iter()
            .find_map(|(variable, location, parameter)| {
                contains_type_variable(&self.resolve(&TypeDescriptor::Inference(*variable))).then(
                    || {
                        (
                            *location,
                            format!("cannot infer type argument `_` for parameter {parameter:?}"),
                        )
                    },
                )
            })
    }

    fn recursive_closure_skeleton(&mut self, expression: &Expr) -> Option<TypeDescriptor> {
        let ExprKind::Closure {
            parameters,
            result_annotation,
            ..
        } = &expression.value
        else {
            return None;
        };
        let parameters = parameters
            .iter()
            .map(|parameter| {
                parameter
                    .annotation
                    .as_ref()
                    .and_then(|annotation| self.local_annotations.get(&annotation.location))
                    .cloned()
                    .unwrap_or_else(|| self.fresh_variable())
            })
            .collect();
        let result = result_annotation
            .as_ref()
            .and_then(|annotation| self.local_annotations.get(&annotation.location))
            .cloned()
            .unwrap_or_else(|| self.fresh_variable());
        Some(TypeDescriptor::Function {
            parameters,
            result: Box::new(result),
        })
    }

    fn recursive_result_variable(descriptor: &TypeDescriptor) -> Option<InferenceVariableId> {
        match descriptor {
            TypeDescriptor::Function { result, .. } => match result.as_ref() {
                TypeDescriptor::Inference(variable) => Some(*variable),
                _ => None,
            },
            _ => None,
        }
    }

    fn recursive_expected(descriptor: &TypeDescriptor) -> TypeDescriptor {
        match descriptor {
            TypeDescriptor::Function { parameters, .. } => TypeDescriptor::Function {
                parameters: parameters.clone(),
                result: Box::new(TypeDescriptor::Any),
            },
            descriptor => descriptor.clone(),
        }
    }

    fn recursive_approximation(
        &self,
        descriptor: &TypeDescriptor,
        variables: &HashSet<InferenceVariableId>,
        approximations: &HashMap<InferenceVariableId, TypeDescriptor>,
    ) -> Option<TypeDescriptor> {
        match descriptor {
            TypeDescriptor::Inference(variable) if variables.contains(variable) => {
                approximations.get(variable).cloned()
            }
            TypeDescriptor::Union(variants) => {
                let resolved = variants
                    .iter()
                    .filter_map(|variant| {
                        self.recursive_approximation(variant, variables, approximations)
                    })
                    .collect::<Vec<_>>();
                (!resolved.is_empty()).then(|| canonical_union(resolved))
            }
            descriptor => {
                let resolved = self.resolve(descriptor);
                (!contains_any_inference_variable(&resolved, variables)).then_some(resolved)
            }
        }
    }

    fn solve_recursive_equations(
        &mut self,
        variables: &HashSet<InferenceVariableId>,
    ) -> Result<(), String> {
        let mut approximations = HashMap::new();
        for _ in 0..=variables.len() {
            let mut changed = false;
            let mut next = approximations.clone();
            for variable in variables {
                let Some(equation) = self.recursive_equations.get(variable) else {
                    continue;
                };
                if let Some(value) =
                    self.recursive_approximation(equation, variables, &approximations)
                    && next.get(variable) != Some(&value)
                {
                    next.insert(*variable, value);
                    changed = true;
                }
            }
            approximations = next;
            if !changed {
                break;
            }
        }
        for (variable, approximation) in approximations {
            self.bind_inference_variable(variable, &approximation)?;
        }
        Ok(())
    }

    fn instantiate_with(
        &mut self,
        ty: &TypeDescriptor,
        variables: &mut HashMap<TypeParameterId, InferenceVariableId>,
    ) -> TypeDescriptor {
        match ty {
            TypeDescriptor::Bound(parameter) => variables
                .get(parameter)
                .map_or_else(|| ty.clone(), |fresh| TypeDescriptor::Inference(*fresh)),
            TypeDescriptor::Declared(declared) => {
                let arguments = declared
                    .id
                    .arguments()
                    .iter()
                    .map(|argument| self.instantiate_with(argument, variables))
                    .collect::<Vec<_>>();
                TypeDescriptor::Declared(DeclaredTypeDescriptor {
                    id: declared.id.reapply(&arguments),
                    name: declared.name.clone(),
                    body: if arguments.is_empty() {
                        Arc::clone(&declared.body)
                    } else {
                        Arc::new(self.instantiate_with(&declared.body, variables))
                    },
                })
            }
            TypeDescriptor::Array(item) => {
                TypeDescriptor::Array(Box::new(self.instantiate_with(item, variables)))
            }
            TypeDescriptor::Dict(item) => {
                TypeDescriptor::Dict(Box::new(self.instantiate_with(item, variables)))
            }
            TypeDescriptor::TypeOf(instance) => {
                TypeDescriptor::TypeOf(Box::new(self.instantiate_with(instance, variables)))
            }
            TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
                tag: tag.clone(),
                payload: Box::new(self.instantiate_with(payload, variables)),
            },
            TypeDescriptor::Tuple(items) => TypeDescriptor::Tuple(
                items
                    .iter()
                    .map(|item| self.instantiate_with(item, variables))
                    .collect(),
            ),
            TypeDescriptor::Struct(fields) => {
                let mut instantiated = fields.clone();
                for (source, target) in fields.values().zip(instantiated.values_mut()) {
                    *target = self.instantiate_with(source, variables);
                }
                TypeDescriptor::Struct(instantiated)
            }
            TypeDescriptor::Enum(variants) => {
                let mut instantiated = variants.clone();
                for (source, target) in variants.values().zip(instantiated.values_mut()) {
                    *target = source
                        .as_ref()
                        .map(|payload| Box::new(self.instantiate_with(payload, variables)));
                }
                TypeDescriptor::Enum(instantiated)
            }
            TypeDescriptor::Union(variants) => TypeDescriptor::Union(
                variants
                    .iter()
                    .map(|variant| self.instantiate_with(variant, variables))
                    .collect(),
            ),
            TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
                parameters: parameters
                    .iter()
                    .map(|parameter| self.instantiate_with(parameter, variables))
                    .collect(),
                result: Box::new(self.instantiate_with(result, variables)),
            },
            ty => ty.clone(),
        }
    }

    fn resolve(&self, ty: &TypeDescriptor) -> TypeDescriptor {
        match ty {
            TypeDescriptor::Inference(variable) => self
                .substitutions
                .get(variable)
                .map_or_else(|| ty.clone(), |ty| self.resolve(ty)),
            TypeDescriptor::Declared(declared) => {
                let arguments = declared
                    .id
                    .arguments()
                    .iter()
                    .map(|argument| self.resolve(argument))
                    .collect::<Vec<_>>();
                TypeDescriptor::Declared(DeclaredTypeDescriptor {
                    id: declared.id.reapply(&arguments),
                    name: declared.name.clone(),
                    body: if arguments.is_empty() {
                        Arc::clone(&declared.body)
                    } else {
                        Arc::new(self.resolve(&declared.body))
                    },
                })
            }
            TypeDescriptor::Array(item) => TypeDescriptor::Array(Box::new(self.resolve(item))),
            TypeDescriptor::Dict(item) => TypeDescriptor::Dict(Box::new(self.resolve(item))),
            TypeDescriptor::TypeOf(instance) => {
                TypeDescriptor::TypeOf(Box::new(self.resolve(instance)))
            }
            TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
                tag: tag.clone(),
                payload: Box::new(self.resolve(payload)),
            },
            TypeDescriptor::Tuple(items) => {
                TypeDescriptor::Tuple(items.iter().map(|item| self.resolve(item)).collect())
            }
            TypeDescriptor::Struct(fields) => {
                let mut resolved = fields.clone();
                for (source, target) in fields.values().zip(resolved.values_mut()) {
                    *target = self.resolve(source);
                }
                TypeDescriptor::Struct(resolved)
            }
            TypeDescriptor::Enum(variants) => {
                let mut resolved = variants.clone();
                for (source, target) in variants.values().zip(resolved.values_mut()) {
                    *target = source
                        .as_ref()
                        .map(|payload| Box::new(self.resolve(payload)));
                }
                TypeDescriptor::Enum(resolved)
            }
            TypeDescriptor::Union(variants) => {
                let variants = variants
                    .iter()
                    .map(|variant| self.resolve(variant))
                    .collect::<Vec<_>>();
                canonical_union(variants)
            }
            TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
                parameters: parameters
                    .iter()
                    .map(|parameter| self.resolve(parameter))
                    .collect(),
                result: Box::new(self.resolve(result)),
            },
            ty => ty.clone(),
        }
    }

    fn occurs(&self, variable: InferenceVariableId, ty: &TypeDescriptor) -> bool {
        match self.resolve(ty) {
            TypeDescriptor::Inference(candidate) => candidate == variable,
            TypeDescriptor::Declared(declared) => {
                declared
                    .id
                    .arguments()
                    .iter()
                    .any(|argument| self.occurs(variable, argument))
                    || self.occurs(variable, &declared.body)
            }
            TypeDescriptor::Array(item) => self.occurs(variable, &item),
            TypeDescriptor::Dict(item) => self.occurs(variable, &item),
            TypeDescriptor::TypeOf(instance) => self.occurs(variable, &instance),
            TypeDescriptor::Tagged { payload, .. } => self.occurs(variable, &payload),
            TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
                items.iter().any(|item| self.occurs(variable, item))
            }
            TypeDescriptor::Struct(fields) => {
                fields.values().any(|field| self.occurs(variable, field))
            }
            TypeDescriptor::Enum(variants) => variants
                .values()
                .flatten()
                .any(|payload| self.occurs(variable, payload)),
            TypeDescriptor::Function { parameters, result } => {
                parameters
                    .iter()
                    .any(|parameter| self.occurs(variable, parameter))
                    || self.occurs(variable, &result)
            }
            _ => false,
        }
    }

    fn require_numeric(&mut self, ty: &TypeDescriptor) -> Result<(), String> {
        match self.resolve(ty) {
            TypeDescriptor::Inference(variable) => {
                self.numeric_variables.insert(variable);
                Ok(())
            }
            TypeDescriptor::Int
            | TypeDescriptor::Float
            | TypeDescriptor::Any
            | TypeDescriptor::Never => Ok(()),
            ty => Err(format!(
                "numeric operator requires Int or Float, found {}",
                ty.display_name()
            )),
        }
    }

    fn require_not_operand(&mut self, ty: &TypeDescriptor) -> Result<(), String> {
        match self.resolve(ty) {
            TypeDescriptor::Inference(variable) => {
                self.not_variables.insert(variable);
                Ok(())
            }
            TypeDescriptor::Int | TypeDescriptor::Any | TypeDescriptor::Never => Ok(()),
            TypeDescriptor::Atom(Atom::Builtin(BuiltinAtom::True | BuiltinAtom::False)) => Ok(()),
            TypeDescriptor::Enum(variants)
                if TypeDescriptor::Enum(variants.clone()) == normalized_bool_descriptor() =>
            {
                Ok(())
            }
            ty => Err(format!(
                "! requires Int or Bool, found {}",
                ty.display_name()
            )),
        }
    }

    fn require_ordered(&mut self, ty: &TypeDescriptor) -> Result<(), String> {
        match self.resolve(ty) {
            TypeDescriptor::Inference(variable) => {
                self.ordered_variables.insert(variable);
                Ok(())
            }
            TypeDescriptor::Int
            | TypeDescriptor::Float
            | TypeDescriptor::String
            | TypeDescriptor::Any
            | TypeDescriptor::Never => Ok(()),
            ty => Err(format!(
                "ordered comparison requires Int, Float, or String, found {}",
                ty.display_name()
            )),
        }
    }

    fn bind_inference_variable(
        &mut self,
        variable: InferenceVariableId,
        ty: &TypeDescriptor,
    ) -> Result<(), String> {
        let ty = self.resolve(ty);
        if self.occurs(variable, &ty) {
            return Err(format!("infinite type for ?{}", variable.0));
        }
        if self.numeric_variables.contains(&variable) {
            match &ty {
                TypeDescriptor::Inference(target) => {
                    self.numeric_variables.insert(*target);
                }
                TypeDescriptor::Int
                | TypeDescriptor::Float
                | TypeDescriptor::Any
                | TypeDescriptor::Never => {}
                _ => {
                    return Err(format!(
                        "numeric operator requires Int or Float, found {}",
                        ty.display_name()
                    ));
                }
            }
        }
        if let TypeDescriptor::Inference(target) = ty
            && self.numeric_variables.contains(&target)
        {
            self.numeric_variables.insert(variable);
        }
        if self.not_variables.contains(&variable) {
            match &ty {
                TypeDescriptor::Inference(target) => {
                    self.not_variables.insert(*target);
                }
                TypeDescriptor::Int | TypeDescriptor::Any | TypeDescriptor::Never => {}
                TypeDescriptor::Atom(Atom::Builtin(BuiltinAtom::True | BuiltinAtom::False)) => {}
                TypeDescriptor::Enum(variants)
                    if TypeDescriptor::Enum(variants.clone()) == normalized_bool_descriptor() => {}
                _ => {
                    return Err(format!(
                        "! requires Int or Bool, found {}",
                        ty.display_name()
                    ));
                }
            }
        }
        if let TypeDescriptor::Inference(target) = ty
            && self.not_variables.contains(&target)
        {
            self.not_variables.insert(variable);
        }
        if self.ordered_variables.contains(&variable) {
            match &ty {
                TypeDescriptor::Inference(target) => {
                    self.ordered_variables.insert(*target);
                }
                TypeDescriptor::Int
                | TypeDescriptor::Float
                | TypeDescriptor::String
                | TypeDescriptor::Any
                | TypeDescriptor::Never => {}
                _ => {
                    return Err(format!(
                        "ordered comparison requires Int, Float, or String, found {}",
                        ty.display_name()
                    ));
                }
            }
        }
        if let TypeDescriptor::Inference(target) = ty
            && self.ordered_variables.contains(&target)
        {
            self.ordered_variables.insert(variable);
        }
        if let Some(requirements) = self.field_requirements.remove(&variable) {
            if let TypeDescriptor::Inference(target) = &ty {
                let mut merged = self.field_requirements.remove(target).unwrap_or_default();
                for (field, result) in requirements {
                    if let Some(existing) = merged.get(&field).cloned() {
                        self.unify(&result, &existing)?;
                    } else {
                        merged.insert(field, result);
                    }
                }
                self.field_requirements.insert(*target, merged);
            } else {
                for (field, result) in requirements {
                    let projected = self.project_field(&ty, &field)?;
                    self.check(&projected, &result)?;
                }
            }
        }
        self.substitutions.insert(variable, ty);
        Ok(())
    }

    fn unify(&mut self, left: &TypeDescriptor, right: &TypeDescriptor) -> Result<(), String> {
        if let Some(query) = &self.query {
            query.check().map_err(|error| error.to_string())?;
        }
        if let (Some(left), Some(right)) =
            (self.declared_identity(left), self.declared_identity(right))
            && left.has_same_head(&right)
            && left.arguments().len() == right.arguments().len()
        {
            for (left, right) in left.arguments().iter().zip(right.arguments()) {
                self.unify(left, right)?;
            }
            return Ok(());
        }
        let left = self.resolve(left);
        let right = self.resolve(right);
        if let (TypeDescriptor::Struct(fields), TypeDescriptor::Dict(item)) = (&left, &right) {
            for field in fields.values() {
                self.unify(field, item)?;
            }
            return Ok(());
        }
        if matches!(
            (&left, &right),
            (TypeDescriptor::Dict(_), TypeDescriptor::Struct(_))
        ) {
            return Err(format!(
                "cannot unify {} with {}",
                left.display_name(),
                right.display_name()
            ));
        }
        if !contains_type_variable(&left)
            && !contains_type_variable(&right)
            && (contains_named_type(&left) || contains_named_type(&right))
        {
            return self.check(&left, &right);
        }
        if !contains_type_variable(&left)
            && !contains_type_variable(&right)
            && (assignable(&left, &right) || assignable(&right, &left))
        {
            return Ok(());
        }
        match (&left, &right) {
            (TypeDescriptor::Inference(left), TypeDescriptor::Inference(right))
                if left == right =>
            {
                Ok(())
            }
            (TypeDescriptor::Inference(variable), ty)
            | (ty, TypeDescriptor::Inference(variable)) => {
                self.bind_inference_variable(*variable, ty)
            }
            (TypeDescriptor::Any, _) | (_, TypeDescriptor::Any) => Ok(()),
            (TypeDescriptor::TypeOf(_), TypeDescriptor::Type) => Ok(()),
            (TypeDescriptor::TypeOf(left), TypeDescriptor::TypeOf(right)) => {
                self.unify(left, right)
            }
            (TypeDescriptor::Declared(left), TypeDescriptor::Declared(right))
                if left.id.has_same_head(&right.id)
                    && left.id.arguments().len() == right.id.arguments().len() =>
            {
                for (left, right) in left.id.arguments().iter().zip(right.id.arguments()) {
                    self.unify(left, right)?;
                }
                Ok(())
            }
            (TypeDescriptor::Array(left), TypeDescriptor::Array(right)) => self.unify(left, right),
            (TypeDescriptor::Dict(left), TypeDescriptor::Dict(right)) => self.unify(left, right),
            (
                TypeDescriptor::Tagged {
                    tag: left_tag,
                    payload: left,
                },
                TypeDescriptor::Tagged {
                    tag: right_tag,
                    payload: right,
                },
            ) if left_tag == right_tag => self.unify(left, right),
            (TypeDescriptor::Tagged { tag, payload }, TypeDescriptor::Enum(variants))
            | (TypeDescriptor::Enum(variants), TypeDescriptor::Tagged { tag, payload }) => variants
                .get(tag.name())
                .and_then(Option::as_deref)
                .ok_or_else(|| format!("Enum has no payload variant '{}", tag.name()))
                .and_then(|expected| self.unify(payload, expected)),
            (TypeDescriptor::Atom(tag), TypeDescriptor::Enum(variants))
            | (TypeDescriptor::Enum(variants), TypeDescriptor::Atom(tag))
                if variants.get(tag.name()).is_some_and(Option::is_none) =>
            {
                Ok(())
            }
            (TypeDescriptor::Atom(tag), TypeDescriptor::Function { parameters, result })
            | (TypeDescriptor::Function { parameters, result }, TypeDescriptor::Atom(tag))
                if parameters.len() == 1 =>
            {
                self.unify(
                    &TypeDescriptor::Tagged {
                        tag: tag.clone(),
                        payload: Box::new(parameters[0].clone()),
                    },
                    result,
                )
            }
            (TypeDescriptor::Tuple(left), TypeDescriptor::Tuple(right))
            | (TypeDescriptor::Union(left), TypeDescriptor::Union(right))
                if left.len() == right.len() =>
            {
                for (left, right) in left.iter().zip(right) {
                    self.unify(left, right)?;
                }
                Ok(())
            }
            (TypeDescriptor::Struct(left), TypeDescriptor::Struct(right))
                if left.keys().eq(right.keys()) =>
            {
                for (name, left) in left {
                    self.unify(left, &right[name])?;
                }
                Ok(())
            }
            (TypeDescriptor::Enum(left), TypeDescriptor::Enum(right))
                if left.keys().eq(right.keys()) =>
            {
                for (name, left) in left {
                    match (left.as_deref(), right[name].as_deref()) {
                        (None, None) => {}
                        (Some(left), Some(right)) => self.unify(left, right)?,
                        _ => {
                            return Err(format!("Enum variant {name} payload shape differs"));
                        }
                    }
                }
                Ok(())
            }
            (
                TypeDescriptor::Function {
                    parameters: left_parameters,
                    result: left_result,
                },
                TypeDescriptor::Function {
                    parameters: right_parameters,
                    result: right_result,
                },
            ) if left_parameters.len() == right_parameters.len() => {
                for (left, right) in left_parameters.iter().zip(right_parameters) {
                    self.unify(left, right)?;
                }
                self.unify(left_result, right_result)
            }
            _ if left == right => Ok(()),
            _ => Err(format!(
                "cannot unify {} with {}",
                left.display_name(),
                right.display_name()
            )),
        }
    }

    fn check(&mut self, actual: &TypeDescriptor, expected: &TypeDescriptor) -> Result<(), String> {
        if let (Some(actual), Some(expected)) = (
            self.declared_identity(actual),
            self.declared_identity(expected),
        ) && actual.has_same_head(&expected)
            && actual.arguments().len() == expected.arguments().len()
        {
            for (actual, expected) in actual.arguments().iter().zip(expected.arguments()) {
                self.check(actual, expected)?;
            }
            return Ok(());
        }
        if let (TypeDescriptor::Named(actual), TypeDescriptor::Named(expected)) = (actual, expected)
        {
            if actual == expected {
                return Ok(());
            }
            let pair = (actual.clone(), expected.clone());
            if !self.checking_named_pairs.insert(pair.clone()) {
                return Ok(());
            }
            let actual_body = self.named_type(actual).cloned();
            let expected_body = self.named_type(expected).cloned();
            let result = match (actual_body, expected_body) {
                (Some(actual), Some(expected)) => self.check(&actual, &expected),
                _ => Err(format!(
                    "cannot unify {} with {}",
                    display_named_type(actual),
                    display_named_type(expected)
                )),
            };
            self.checking_named_pairs.remove(&pair);
            return result;
        }
        if let Some(actual_name) = self.named_identity(actual) {
            let pair = (actual_name.clone(), format!("{:?}", expected));
            if !self.checking_named_pairs.insert(pair.clone()) {
                return Ok(());
            }
            let actual_body = self.named_type(&actual_name).cloned();
            let result = actual_body.map_or_else(
                || {
                    Err(format!(
                        "unknown concrete type {}",
                        display_named_type(&actual_name)
                    ))
                },
                |actual| self.check(&actual, expected),
            );
            self.checking_named_pairs.remove(&pair);
            return result;
        }
        if let Some(expected_name) = self.named_identity(expected) {
            let pair = (format!("{:?}", actual), expected_name.clone());
            if !self.checking_named_pairs.insert(pair.clone()) {
                return Ok(());
            }
            let expected_body = self.named_type(&expected_name).cloned();
            let result = expected_body.map_or_else(
                || {
                    Err(format!(
                        "unknown concrete type {}",
                        display_named_type(&expected_name)
                    ))
                },
                |expected| self.check(actual, &expected),
            );
            self.checking_named_pairs.remove(&pair);
            return result;
        }
        let actual = self.expose_named(actual);
        let expected = self.expose_named(expected);
        if matches!(actual, TypeDescriptor::Never) {
            return Ok(());
        }
        if let TypeDescriptor::Inference(variable) = expected
            && contains_runtime_never_leaf(&actual)
        {
            let evidence = self.freshen_runtime_never_leaves(&actual);
            return self.bind_inference_variable(variable, &evidence);
        }
        if contains_type_variable(&actual)
            && let TypeDescriptor::Union(variants) = &expected
        {
            let candidates = variants
                .iter()
                .filter(|variant| potentially_assignable(&actual, variant))
                .collect::<Vec<_>>();
            if let [candidate] = candidates.as_slice() {
                return self.check(&actual, candidate);
            }
        }
        match (&actual, &expected) {
            (TypeDescriptor::Union(variants), TypeDescriptor::Enum(_)) => {
                for variant in variants {
                    self.check(variant, &expected)?;
                }
                return Ok(());
            }
            (TypeDescriptor::Tagged { tag, payload }, TypeDescriptor::Enum(variants)) => {
                let Some(Some(expected_payload)) = variants.get(tag.name()) else {
                    return Err(format!(
                        "cannot unify {} with {}",
                        actual.display_name(),
                        expected.display_name()
                    ));
                };
                return self.check(payload, expected_payload);
            }
            (TypeDescriptor::Atom(tag), TypeDescriptor::Enum(variants)) => {
                if matches!(variants.get(tag.name()), Some(None)) {
                    return Ok(());
                }
            }
            (TypeDescriptor::Array(actual), TypeDescriptor::Array(expected))
            | (TypeDescriptor::Dict(actual), TypeDescriptor::Dict(expected))
            | (TypeDescriptor::TypeOf(actual), TypeDescriptor::TypeOf(expected)) => {
                return self.check(actual, expected);
            }
            (
                TypeDescriptor::Tagged {
                    tag: actual_tag,
                    payload: actual,
                },
                TypeDescriptor::Tagged {
                    tag: expected_tag,
                    payload: expected,
                },
            ) if actual_tag == expected_tag => return self.check(actual, expected),
            (TypeDescriptor::Tuple(actual), TypeDescriptor::Tuple(expected))
                if actual.len() == expected.len() =>
            {
                for (actual, expected) in actual.iter().zip(expected) {
                    self.check(actual, expected)?;
                }
                return Ok(());
            }
            (TypeDescriptor::Struct(actual), TypeDescriptor::Struct(expected))
                if actual.keys().eq(expected.keys()) =>
            {
                for (name, actual) in actual {
                    self.check(actual, &expected[name])?;
                }
                return Ok(());
            }
            (TypeDescriptor::Struct(actual), TypeDescriptor::Dict(expected)) => {
                for actual in actual.values() {
                    self.check(actual, expected)?;
                }
                return Ok(());
            }
            (TypeDescriptor::Enum(actual), TypeDescriptor::Enum(expected))
                if actual.keys().eq(expected.keys()) =>
            {
                for (name, actual_payload) in actual {
                    match (actual_payload, &expected[name]) {
                        (None, None) => {}
                        (Some(actual), Some(expected)) => self.check(actual, expected)?,
                        _ => {
                            return Err(format!("Enum variant {name} payload shape differs"));
                        }
                    }
                }
                return Ok(());
            }
            (
                TypeDescriptor::Function {
                    parameters: actual_parameters,
                    result: actual_result,
                },
                TypeDescriptor::Function {
                    parameters: expected_parameters,
                    result: expected_result,
                },
            ) if actual_parameters.len() == expected_parameters.len() => {
                for (actual, expected) in actual_parameters.iter().zip(expected_parameters) {
                    self.check(actual, expected)?;
                }
                self.check(actual_result, expected_result)?;
                return Ok(());
            }
            _ => {}
        }
        if !contains_type_variable(&actual) && !contains_type_variable(&expected) {
            return assignable(&actual, &expected).then_some(()).ok_or_else(|| {
                format!(
                    "cannot unify {} with {}",
                    actual.display_name(),
                    expected.display_name()
                )
            });
        }
        self.unify(&actual, &expected)
    }

    fn default_inference_variables_to_any(&mut self, ty: &TypeDescriptor) {
        match self.resolve(ty) {
            TypeDescriptor::Inference(variable) => {
                self.substitutions.insert(variable, TypeDescriptor::Any);
            }
            TypeDescriptor::Array(item)
            | TypeDescriptor::Dict(item)
            | TypeDescriptor::TypeOf(item) => {
                self.default_inference_variables_to_any(&item);
            }
            TypeDescriptor::Tagged { payload, .. } => {
                self.default_inference_variables_to_any(&payload);
            }
            TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
                for item in items {
                    self.default_inference_variables_to_any(&item);
                }
            }
            TypeDescriptor::Struct(fields) => {
                for field in fields.values() {
                    self.default_inference_variables_to_any(field);
                }
            }
            TypeDescriptor::Enum(variants) => {
                for payload in variants.values().flatten() {
                    self.default_inference_variables_to_any(payload);
                }
            }
            TypeDescriptor::Function { parameters, result } => {
                for parameter in parameters {
                    self.default_inference_variables_to_any(&parameter);
                }
                self.default_inference_variables_to_any(&result);
            }
            _ => {}
        }
    }

    fn freshen_runtime_never_leaves(&mut self, descriptor: &TypeDescriptor) -> TypeDescriptor {
        match descriptor {
            TypeDescriptor::Never => self.fresh_variable(),
            TypeDescriptor::Array(item) => {
                TypeDescriptor::Array(Box::new(self.freshen_runtime_never_leaves(item)))
            }
            TypeDescriptor::Dict(item) => {
                TypeDescriptor::Dict(Box::new(self.freshen_runtime_never_leaves(item)))
            }
            TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
                tag: tag.clone(),
                payload: Box::new(self.freshen_runtime_never_leaves(payload)),
            },
            TypeDescriptor::Tuple(items) => TypeDescriptor::Tuple(
                items
                    .iter()
                    .map(|item| self.freshen_runtime_never_leaves(item))
                    .collect(),
            ),
            TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
                fields
                    .iter()
                    .map(|(name, field)| (name.clone(), self.freshen_runtime_never_leaves(field)))
                    .collect(),
            ),
            descriptor => descriptor.clone(),
        }
    }

    fn project_field(
        &mut self,
        receiver: &TypeDescriptor,
        field: &str,
    ) -> Result<TypeDescriptor, String> {
        match self.expose_named(receiver) {
            TypeDescriptor::Declared(declared) => self.project_field(&declared.body, field),
            TypeDescriptor::Struct(fields) => fields
                .get(field)
                .cloned()
                .ok_or_else(|| format!("Struct has no field {field:?}")),
            TypeDescriptor::Dict(item) => Ok(*item),
            TypeDescriptor::Union(variants) => variants
                .iter()
                .map(|variant| self.project_field(variant, field))
                .collect::<Result<Vec<_>, _>>()
                .map(join_all_types),
            TypeDescriptor::Never => Ok(TypeDescriptor::Never),
            TypeDescriptor::Any => Ok(TypeDescriptor::Any),
            TypeDescriptor::Inference(variable) => {
                if let Some(result) = self
                    .field_requirements
                    .get(&variable)
                    .and_then(|fields| fields.get(field))
                {
                    return Ok(result.clone());
                }
                let result = self.fresh_variable();
                self.field_requirements
                    .entry(variable)
                    .or_default()
                    .insert(field.to_owned(), result.clone());
                Ok(result)
            }
            descriptor => Err(format!(
                "cannot access field {field:?} on {}",
                descriptor.display_name()
            )),
        }
    }

    fn expose_pattern_type(&self, descriptor: &TypeDescriptor) -> TypeDescriptor {
        match self.expose_named(descriptor) {
            TypeDescriptor::Declared(declared) => self.expose_pattern_type(&declared.body),
            descriptor => descriptor,
        }
    }

    fn project_tuple(
        &mut self,
        receiver: &TypeDescriptor,
        index: usize,
    ) -> Result<TypeDescriptor, String> {
        match self.expose_named(receiver) {
            TypeDescriptor::Tuple(items) => items.get(index).cloned().ok_or_else(|| {
                format!(
                    "Tuple of length {} has no item at index {index}",
                    items.len()
                )
            }),
            TypeDescriptor::Union(variants) => variants
                .iter()
                .map(|variant| self.project_tuple(variant, index))
                .collect::<Result<Vec<_>, _>>()
                .map(canonical_union),
            TypeDescriptor::Never => Ok(TypeDescriptor::Never),
            TypeDescriptor::Any => Ok(TypeDescriptor::Any),
            descriptor => Err(format!(
                "cannot project tuple item {index} from {}",
                descriptor.display_name()
            )),
        }
    }

    fn materialize_field_requirements(
        &mut self,
        descriptor: &TypeDescriptor,
    ) -> Result<(), String> {
        let mut variables = Vec::new();
        collect_inference_variables(&self.resolve(descriptor), &mut variables);
        variables.sort_unstable();
        variables.dedup();
        for variable in variables {
            let Some(fields) = self.field_requirements.remove(&variable) else {
                continue;
            };
            self.bind_inference_variable(variable, &TypeDescriptor::Struct(fields))?;
        }
        Ok(())
    }

    fn infer(
        &mut self,
        expression: &Expr,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        let declared_construction = expected.and_then(|expected| {
            let TypeDescriptor::Declared(declared) = self.expose_named(expected) else {
                return None;
            };
            let constructible = declared_body_accepts_expression(&declared.body, expression);
            constructible.then_some(declared)
        });
        let structural_expected = declared_construction
            .as_ref()
            .map(|declared| declared.body.as_ref());
        let result = self.infer_inner(expression, environment, structural_expected.or(expected));
        let result = result.map(|inferred| {
            declared_construction.map_or(inferred, |declared| {
                let declared = TypeDescriptor::Declared(declared);
                self.records.insert(expression.location, declared.clone());
                declared
            })
        });
        if result.is_err() && self.failure_location.is_none() {
            self.failure_location = Some(expression.location);
        }
        result
    }

    fn infer_inner(
        &mut self,
        expression: &Expr,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        if let Some(query) = &self.query {
            query.check().map_err(|error| error.to_string())?;
        }
        let inferred = match &expression.value {
            ExprKind::Variable(name) => self.scheme(&name.value).map_or_else(
                || {
                    environment
                        .get(&name.value)
                        .cloned()
                        .unwrap_or(TypeDescriptor::Any)
                },
                |scheme| self.instantiate(&scheme),
            ),
            ExprKind::Int(_) => TypeDescriptor::Int,
            ExprKind::Float(_) => TypeDescriptor::Float,
            ExprKind::String(_) => TypeDescriptor::String,
            ExprKind::InterpolatedString(parts) => {
                for part in parts {
                    if let StringPartKind::Expression(expression) = &part.value {
                        self.infer(expression, environment, None)?;
                    }
                }
                TypeDescriptor::String
            }
            ExprKind::Bytes(_) => TypeDescriptor::Bytes,
            ExprKind::Atom(name) => TypeDescriptor::Atom(atom_from_name(name)),
            ExprKind::Array(items) => {
                let item_expected = match expected.map(|ty| self.resolve(ty)) {
                    Some(TypeDescriptor::Array(item))
                        if items.is_empty()
                            || !matches!(self.resolve(&item), TypeDescriptor::Inference(_)) =>
                    {
                        Some(*item)
                    }
                    _ => None,
                };
                let mut item_types = Vec::new();
                for item in items {
                    if let ExprKind::Spread(operand) = &item.value {
                        let spread_expected = item_expected
                            .as_ref()
                            .map(|item| TypeDescriptor::Array(Box::new(item.clone())));
                        let spread = self.infer(operand, environment, spread_expected.as_ref())?;
                        let resolved = self.resolve(&spread);
                        let TypeDescriptor::Array(spread_item) = resolved else {
                            return Err(format!(
                                "array spread requires Array, found {}",
                                resolved.display_name()
                            ));
                        };
                        item_types.push(*spread_item);
                    } else {
                        item_types.push(self.infer(item, environment, item_expected.as_ref())?);
                    }
                }
                let item = if let Some(expected) = item_expected {
                    expected
                } else if items.is_empty() && self.delayed_initializer_depth > 0 {
                    self.fresh_variable()
                } else {
                    join_all_types(item_types)
                };
                TypeDescriptor::Array(Box::new(item))
            }
            ExprKind::Spread(operand) => self.infer(operand, environment, expected)?,
            ExprKind::Tuple(items) => {
                let item_expected = match expected.map(|ty| self.resolve(ty)) {
                    Some(TypeDescriptor::Tuple(expected_items))
                        if expected_items.len() == items.len() =>
                    {
                        expected_items
                    }
                    _ => Vec::new(),
                };
                TypeDescriptor::Tuple(
                    items
                        .iter()
                        .enumerate()
                        .map(|(index, item)| {
                            self.infer(item, environment, item_expected.get(index))
                        })
                        .collect::<Result<_, _>>()?,
                )
            }
            ExprKind::Dict(fields) => {
                let has_spread = fields.iter().any(|field| field.value.name.is_none());
                let metadata_expected = expected
                    .map(|ty| self.resolve(ty))
                    .filter(|ty| matches!(ty, TypeDescriptor::Type | TypeDescriptor::TypeOf(_)));
                if let Some(metadata_expected) = metadata_expected {
                    if has_spread {
                        return Err("Dict spread is not valid in type metadata".into());
                    }
                    for field in fields {
                        self.infer(&field.value.value, environment, None)?;
                    }
                    metadata_expected
                } else if has_spread {
                    let item_expected = match expected.map(|ty| self.resolve(ty)) {
                        Some(TypeDescriptor::Dict(item)) => Some(*item),
                        _ => None,
                    };
                    let mut item_types = Vec::new();
                    for field in fields {
                        if field.value.name.is_none() {
                            let ExprKind::Spread(operand) = &field.value.value.value else {
                                return Err("invalid Dict spread entry".into());
                            };
                            let spread_expected = item_expected
                                .as_ref()
                                .map(|item| TypeDescriptor::Dict(Box::new(item.clone())));
                            let spread =
                                self.infer(operand, environment, spread_expected.as_ref())?;
                            let resolved = self.resolve(&spread);
                            let TypeDescriptor::Dict(spread_item) = resolved else {
                                return Err(format!(
                                    "Dict spread requires Dict, found {}",
                                    resolved.display_name()
                                ));
                            };
                            item_types.push(*spread_item);
                        } else {
                            item_types.push(self.infer(
                                &field.value.value,
                                environment,
                                item_expected.as_ref(),
                            )?);
                        }
                    }
                    TypeDescriptor::Dict(Box::new(
                        item_expected.unwrap_or_else(|| join_all_types(item_types)),
                    ))
                } else {
                    if let Some(TypeDescriptor::Dict(item)) = expected.map(|ty| self.resolve(ty)) {
                        for field in fields {
                            self.infer(&field.value.value, environment, Some(&item))
                                .map_err(|message| {
                                    format!(
                                        "field {}: {message}",
                                        field
                                            .value
                                            .name
                                            .as_ref()
                                            .expect("ordinary Dict field has a name")
                                            .value
                                    )
                                })?;
                        }
                        TypeDescriptor::Dict(item)
                    } else {
                        let expected_fields = match expected.map(|ty| self.resolve(ty)) {
                            Some(TypeDescriptor::Struct(fields)) => fields,
                            _ => BTreeMap::new(),
                        };
                        TypeDescriptor::Struct(
                            fields
                                .iter()
                                .map(|field| {
                                    let name = field
                                        .value
                                        .name
                                        .as_ref()
                                        .expect("ordinary Dict field has a name")
                                        .value
                                        .clone();
                                    Ok((
                                        name.clone(),
                                        self.infer(
                                            &field.value.value,
                                            environment,
                                            expected_fields.get(&name),
                                        )
                                        .map_err(|message| format!("field {name}: {message}"))?,
                                    ))
                                })
                                .collect::<Result<_, String>>()?,
                        )
                    }
                }
            }
            ExprKind::Unary { operator, operand } => match operator.value {
                UnaryOperator::Negate => {
                    let numeric = self.fresh_variable();
                    self.require_numeric(&numeric)?;
                    if let Some(expected) = expected {
                        self.check(&numeric, expected)?;
                    }
                    let operand = self.infer(operand, environment, Some(&numeric))?;
                    self.require_numeric(&operand)?;
                    self.resolve(&numeric)
                }
                UnaryOperator::Not => {
                    let resolved_expected = expected.map(|expected| self.resolve(expected));
                    let expected_family =
                        resolved_expected
                            .as_ref()
                            .and_then(|expected| match expected {
                                TypeDescriptor::Int => Some(NotFamily::Int),
                                TypeDescriptor::Enum(variants)
                                    if TypeDescriptor::Enum(variants.clone())
                                        == normalized_bool_descriptor() =>
                                {
                                    Some(NotFamily::Bool)
                                }
                                TypeDescriptor::Any => Some(NotFamily::Dynamic),
                                _ => None,
                            });
                    let operand_expectation = resolved_expected.as_ref().filter(|expected| {
                        matches!(expected, TypeDescriptor::Int | TypeDescriptor::Any)
                            || matches!(
                                expected,
                                TypeDescriptor::Enum(variants)
                                    if TypeDescriptor::Enum(variants.clone())
                                        == normalized_bool_descriptor()
                            )
                    });
                    let operand = self.infer(operand, environment, operand_expectation)?;
                    self.require_not_operand(&operand)?;
                    let resolved_operand = self.resolve(&operand);
                    let family = expected_family.unwrap_or(match &resolved_operand {
                        TypeDescriptor::Int => NotFamily::Int,
                        TypeDescriptor::Atom(Atom::Builtin(
                            BuiltinAtom::True | BuiltinAtom::False,
                        ))
                        | TypeDescriptor::Enum(_) => NotFamily::Bool,
                        _ => NotFamily::Dynamic,
                    });
                    self.not_families.insert(expression.location, family);
                    let result = match family {
                        NotFamily::Bool => normalized_bool_descriptor(),
                        NotFamily::Int => TypeDescriptor::Int,
                        NotFamily::Dynamic => resolved_operand,
                    };
                    if let Some(expected) = expected {
                        self.check(&result, expected)?;
                    }
                    result
                }
                UnaryOperator::LogicalNot => {
                    let bool_type = normalized_bool_descriptor();
                    self.infer(operand, environment, Some(&bool_type))?;
                    bool_type
                }
                UnaryOperator::BitNot => {
                    self.infer(operand, environment, Some(&TypeDescriptor::Int))?;
                    TypeDescriptor::Int
                }
            },
            ExprKind::Propagate { operand } => {
                let operand = self.infer(operand, environment, None)?;
                match self.resolve(&operand) {
                    TypeDescriptor::Enum(variants) => {
                        if let Some(payload) = option_parts(&variants) {
                            self.propagation_families
                                .insert(expression.location, PropagationFamily::Option);
                            self.record_propagation(PropagationRequirement::Option)?;
                            payload.clone()
                        } else if let Some((ok, err)) =
                            result_parts(&TypeDescriptor::Enum(variants))
                        {
                            self.propagation_families
                                .insert(expression.location, PropagationFamily::Result);
                            let ok = ok.clone();
                            let err = err.clone();
                            self.record_propagation(PropagationRequirement::Result(vec![err]))?;
                            ok
                        } else {
                            return Err(
                                "? operand must be an exact Option-shaped or Result-shaped Enum"
                                    .into(),
                            );
                        }
                    }
                    descriptor => {
                        return Err(format!(
                            "? operand must resolve to an Option-shaped or Result-shaped Enum, found {}",
                            descriptor.display_name()
                        ));
                    }
                }
            }
            ExprKind::Return { value } => {
                let expected = self
                    .return_boundaries
                    .last()
                    .and_then(Option::as_ref)
                    .ok_or_else(|| "return is allowed only inside a Function".to_owned())?
                    .expected
                    .clone();
                let value = self.infer(value, environment, expected.as_ref())?;
                self.return_boundaries
                    .last_mut()
                    .and_then(Option::as_mut)
                    .expect("Function return boundary exists")
                    .values
                    .push(value);
                TypeDescriptor::Never
            }
            ExprKind::Panic { message } => {
                self.infer(message, environment, Some(&TypeDescriptor::String))?;
                TypeDescriptor::Never
            }
            ExprKind::Raise { error } => {
                self.infer(error, environment, Some(&blame_error_descriptor()))?;
                TypeDescriptor::Never
            }
            ExprKind::Debug { value, .. } => self.infer(value, environment, expected)?,
            ExprKind::Binary {
                operator,
                left,
                right,
            } => match operator.value {
                BinaryOperator::And | BinaryOperator::Or => {
                    let bool_type = normalized_bool_descriptor();
                    self.infer(left, environment, Some(&bool_type))?;
                    self.infer(right, environment, Some(&bool_type))?;
                    bool_type
                }
                BinaryOperator::Equal | BinaryOperator::NotEqual => {
                    self.infer(left, environment, None)?;
                    self.infer(right, environment, None)?;
                    normalized_bool_descriptor()
                }
                BinaryOperator::BitAnd | BinaryOperator::BitOr | BinaryOperator::BitXor => {
                    if let Some(expected) = expected {
                        self.check(&TypeDescriptor::Int, expected)?;
                    }
                    self.infer(left, environment, Some(&TypeDescriptor::Int))?;
                    self.infer(right, environment, Some(&TypeDescriptor::Int))?;
                    TypeDescriptor::Int
                }
                BinaryOperator::LessThan
                | BinaryOperator::LessThanOrEqual
                | BinaryOperator::GreaterThan
                | BinaryOperator::GreaterThanOrEqual => {
                    let ordered = self.fresh_variable();
                    self.require_ordered(&ordered)?;
                    let left = self.infer(left, environment, Some(&ordered))?;
                    let right = self.infer(right, environment, Some(&ordered))?;
                    self.require_ordered(&left)?;
                    self.require_ordered(&right)?;
                    normalized_bool_descriptor()
                }
                _ => {
                    let numeric = self.fresh_variable();
                    self.require_numeric(&numeric)?;
                    if let Some(expected) = expected {
                        self.check(&numeric, expected)?;
                    }
                    let left = self.infer(left, environment, Some(&numeric))?;
                    let right = self.infer(right, environment, Some(&numeric))?;
                    self.require_numeric(&left)?;
                    self.require_numeric(&right)?;
                    self.resolve(&numeric)
                }
            },
            ExprKind::Field { receiver, field } => {
                if let ExprKind::Variable(module) = &receiver.value
                    && let Some(scheme) = self
                        .external_interfaces
                        .get(&module.value)
                        .and_then(|interface| interface.exports.get(&field.value))
                        .cloned()
                {
                    self.infer(receiver, environment, None)?;
                    self.instantiate(&scheme)
                } else {
                    let receiver = self.infer(receiver, environment, None)?;
                    self.project_field(&receiver, &field.value)?
                }
            }
            ExprKind::Index { receiver, index } => {
                let receiver = self.infer(receiver, environment, None)?;
                let receiver = self.expose_named(&receiver);
                let result = match receiver {
                    TypeDescriptor::Array(item) => *item,
                    TypeDescriptor::Inference(variable) => {
                        let item = self.fresh_variable();
                        self.bind_inference_variable(
                            variable,
                            &TypeDescriptor::Array(Box::new(item.clone())),
                        )?;
                        item
                    }
                    TypeDescriptor::Never => TypeDescriptor::Never,
                    TypeDescriptor::Any => TypeDescriptor::Any,
                    descriptor => {
                        return Err(format!(
                            "cannot index value of type {}",
                            descriptor.display_name()
                        ));
                    }
                };
                self.infer(index, environment, Some(&TypeDescriptor::Int))?;
                result
            }
            ExprKind::TupleProjection { receiver, index } => {
                let receiver = self.infer(receiver, environment, None)?;
                self.project_tuple(&receiver, index.value)?
            }
            ExprKind::Call { callee, arguments } => {
                if self.is_builtin_tuple(callee)
                    && let [argument] = arguments.as_slice()
                    && let ExprKind::Array(items) = &argument.value
                    && items
                        .iter()
                        .all(|item| !matches!(item.value, ExprKind::Spread(_)))
                {
                    self.infer(callee, environment, None)?;
                    let metadata_array = TypeDescriptor::Array(Box::new(TypeDescriptor::Type));
                    self.infer(argument, environment, Some(&metadata_array))?;
                    let mut tuple_items = Vec::with_capacity(items.len());
                    for item in items {
                        let item = self
                            .records
                            .get(&item.location)
                            .map(|item| self.resolve(item))
                            .ok_or_else(|| "Tuple item has no inferred Type metadata".to_owned())?;
                        match item {
                            TypeDescriptor::TypeOf(item) => tuple_items.push(*item),
                            TypeDescriptor::Type | TypeDescriptor::Any => {
                                tuple_items.push(TypeDescriptor::Any)
                            }
                            item => {
                                return Err(format!(
                                    "Tuple items must be Type metadata, found {}",
                                    item.display_name()
                                ));
                            }
                        };
                    }
                    let inferred =
                        TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Tuple(tuple_items)));
                    if let Some(expected) = expected {
                        self.check(&inferred, expected)?;
                    }
                    let inferred = self.resolve(&inferred);
                    self.records.insert(expression.location, inferred.clone());
                    return Ok(inferred);
                }
                let has_placeholder = matches!(
                    &callee.value,
                    ExprKind::TypeApply { arguments, .. }
                        if arguments
                            .iter()
                            .any(|argument| matches!(argument.value, TypeArgumentKind::Infer))
                );
                let callee = self.infer(callee, environment, None)?;
                let resolved_callee = self.resolve(&callee);
                let resolved_callee = if let TypeDescriptor::Inference(variable) = resolved_callee {
                    let function = TypeDescriptor::Function {
                        parameters: arguments.iter().map(|_| self.fresh_variable()).collect(),
                        result: Box::new(self.fresh_variable()),
                    };
                    self.bind_inference_variable(variable, &function)?;
                    function
                } else {
                    resolved_callee
                };
                match resolved_callee {
                    TypeDescriptor::Atom(tag) => {
                        if arguments.len() != 1 {
                            return Err(format!(
                                "tag constructor expects 1 argument, found {}",
                                arguments.len()
                            ));
                        }
                        let payload_expected = expected
                            .map(|expected| self.resolve(expected))
                            .and_then(|expected| match expected {
                                TypeDescriptor::Enum(variants) => {
                                    variants.get(tag.name()).and_then(|payload| payload.clone())
                                }
                                _ => None,
                            });
                        let payload =
                            self.infer(&arguments[0], environment, payload_expected.as_deref())?;
                        let result = TypeDescriptor::Tagged {
                            tag,
                            payload: Box::new(payload),
                        };
                        if let Some(expected) = expected {
                            self.check(&result, expected)?;
                        }
                        self.resolve(&result)
                    }
                    TypeDescriptor::Function { parameters, result } => {
                        if parameters.len() != arguments.len() {
                            return Err(format!(
                                "call expects {} arguments, found {}",
                                parameters.len(),
                                arguments.len()
                            ));
                        }
                        if let Some(expected) = expected {
                            self.check(&result, expected)?;
                        }
                        let mut partial_tagged_evidence = false;
                        let mut unresolved_argument_evidence = false;
                        let mut argument_order = (0..arguments.len()).collect::<Vec<_>>();
                        argument_order.sort_by_key(|index| match &arguments[*index].value {
                            _ if self.explicit_scheme(&arguments[*index]).is_some() => 0,
                            ExprKind::Dict(_) => 2,
                            ExprKind::Atom(_) => 3,
                            _ => 1,
                        });
                        for index in argument_order {
                            let argument = &arguments[index];
                            let parameter = &parameters[index];
                            let argument_type =
                                self.infer(argument, environment, Some(parameter))?;
                            unresolved_argument_evidence |=
                                contains_type_variable(&self.resolve(&argument_type));
                            partial_tagged_evidence |= matches!(
                                self.resolve(&argument_type),
                                TypeDescriptor::Tagged { .. }
                            );
                            if matches!(self.resolve(&argument_type), TypeDescriptor::Any) {
                                self.default_inference_variables_to_any(parameter);
                            }
                            self.check(&argument_type, parameter)?;
                        }
                        self.materialize_field_requirements(&TypeDescriptor::Tuple(
                            parameters.clone(),
                        ))?;
                        if partial_tagged_evidence {
                            for parameter in &parameters {
                                self.default_inference_variables_to_any(parameter);
                            }
                        }
                        let result = self.resolve(&result);
                        let result = if matches!(result, TypeDescriptor::TypeOf(_))
                            && contains_type_variable(&result)
                        {
                            TypeDescriptor::Type
                        } else {
                            result
                        };
                        if contains_type_variable(&result)
                            && self.delayed_initializer_depth == 0
                            && !has_placeholder
                            && expected.is_none()
                            && !(self.closure_inference_depth > 0 && unresolved_argument_evidence)
                        {
                            return Err(format!(
                                "cannot infer generic result type {}",
                                result.display_name()
                            ));
                        }
                        result
                    }
                    TypeDescriptor::Any => {
                        for argument in arguments {
                            self.infer(argument, environment, None)?;
                        }
                        TypeDescriptor::Any
                    }
                    descriptor => {
                        for argument in arguments {
                            self.infer(argument, environment, None)?;
                        }
                        return Err(format!(
                            "cannot call value of type {}",
                            descriptor.display_name()
                        ));
                    }
                }
            }
            ExprKind::TypeApply { callee, arguments } => {
                let scheme = self.explicit_scheme(callee).ok_or_else(|| {
                    "explicit type application requires a statically known generic binding"
                        .to_owned()
                })?;
                if scheme.parameters.is_empty() {
                    return Err("cannot apply type arguments to a monomorphic binding".into());
                }
                if scheme.parameters.len() != arguments.len() {
                    return Err(format!(
                        "type application expects {} arguments, found {}",
                        scheme.parameters.len(),
                        arguments.len()
                    ));
                }
                self.infer(callee, environment, None)?;
                let type_expected = TypeDescriptor::Type;
                let mut replacements = HashMap::new();
                for (parameter, argument) in scheme.parameters.iter().zip(arguments) {
                    let descriptor = match &argument.value {
                        TypeArgumentKind::Explicit(expression) => {
                            self.infer(expression, environment, Some(&type_expected))?;
                            self.local_annotations
                                .get(&expression.location)
                                .cloned()
                                .ok_or_else(|| {
                                    "type argument metadata was not evaluated".to_owned()
                                })?
                        }
                        TypeArgumentKind::Infer => {
                            let descriptor = self.fresh_variable();
                            let TypeDescriptor::Inference(variable) = &descriptor else {
                                unreachable!("fresh variables are inference descriptors")
                            };
                            self.placeholder_obligations.push((
                                *variable,
                                argument.location,
                                parameter.name.clone(),
                            ));
                            self.records.insert(argument.location, descriptor.clone());
                            descriptor
                        }
                    };
                    replacements.insert(parameter.id, descriptor);
                }
                substitute_bound_parameters(&scheme.body, &replacements)
            }
            ExprKind::Interpreter { elaboration, .. } => {
                self.infer(elaboration, environment, expected)?
            }
            ExprKind::Closure {
                parameters,
                result_annotation,
                body,
            } => {
                let expected = match expected.map(|ty| self.resolve(ty)) {
                    Some(TypeDescriptor::Function {
                        parameters: expected_parameters,
                        result,
                    }) if expected_parameters.len() == parameters.len() => {
                        Some((expected_parameters, result))
                    }
                    _ => None,
                };
                let mut closure_environment = environment.clone();
                let mut parameter_types = Vec::with_capacity(parameters.len());
                for (index, parameter) in parameters.iter().enumerate() {
                    let surrounding = expected
                        .as_ref()
                        .and_then(|(parameters, _)| parameters.get(index));
                    let local = parameter.annotation.as_ref().and_then(|annotation| {
                        self.local_annotations.get(&annotation.location).cloned()
                    });
                    if let (Some(local), Some(surrounding)) = (&local, surrounding) {
                        self.check(local, surrounding)?;
                    }
                    parameter_types.push(
                        local
                            .or_else(|| surrounding.cloned())
                            .unwrap_or_else(|| self.fresh_variable()),
                    );
                }
                for (parameter, ty) in parameters.iter().zip(&parameter_types) {
                    closure_environment.insert(parameter.name.value.clone(), ty.clone());
                }
                let surrounding_result = (self.recursive_body_inference_depth == 0)
                    .then(|| expected.as_ref().map(|(_, result)| result.as_ref()))
                    .flatten();
                let local_result = result_annotation.as_ref().and_then(|annotation| {
                    self.local_annotations.get(&annotation.location).cloned()
                });
                if let (Some(local), Some(surrounding)) = (&local_result, surrounding_result) {
                    self.check(local, surrounding)?;
                }
                let result_expected = local_result.as_ref().or(surrounding_result);
                let inferring_unannotated = expected.is_none();
                if inferring_unannotated {
                    self.closure_inference_depth += 1;
                }
                self.scheme_scopes.push(
                    parameters
                        .iter()
                        .map(|parameter| (parameter.name.value.clone(), None))
                        .collect(),
                );
                self.propagation_boundaries.push(None);
                self.return_boundaries.push(Some(ReturnBoundary {
                    expected: result_expected.cloned(),
                    values: Vec::new(),
                }));
                let result = self.infer_block(body, &closure_environment, result_expected);
                self.scheme_scopes.pop();
                let return_boundary = self
                    .return_boundaries
                    .pop()
                    .and_then(|boundary| boundary)
                    .expect("closure return boundary exists");
                let requirement = self
                    .propagation_boundaries
                    .pop()
                    .expect("closure boundary exists");
                if inferring_unannotated {
                    self.closure_inference_depth -= 1;
                }
                let inferred_result =
                    self.finish_propagation_boundary(result?, result_expected, requirement)?;
                let inferred_result =
                    self.finish_return_boundary(inferred_result, return_boundary)?;
                let function = TypeDescriptor::Function {
                    parameters: parameter_types,
                    result: Box::new(local_result.unwrap_or(inferred_result)),
                };
                if expected.is_none()
                    && self.delayed_initializer_depth == 0
                    && contains_type_variable(&function)
                {
                    self.default_inference_variables_to_any(&function);
                }
                self.resolve(&function)
            }
            ExprKind::Block(block) => self.infer_block(block, environment, expected)?,
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let bool_type = normalized_bool_descriptor();
                self.infer(condition, environment, Some(&bool_type))?;
                let (then_type, else_type) = if let Some(expected) = expected
                    && contains_type_variable(&self.resolve(expected))
                {
                    let (then_expected, then_environment, then_evidence) =
                        self.freshen_join_context(expected, environment);
                    let then_type =
                        self.infer_block(then_branch, &then_environment, Some(&then_expected))?;
                    let (else_expected, else_environment, else_evidence) =
                        self.freshen_join_context(expected, environment);
                    let else_type =
                        self.infer_block(else_branch, &else_environment, Some(&else_expected))?;
                    self.merge_join_evidence(&[then_evidence, else_evidence])?;
                    (then_type, else_type)
                } else {
                    (
                        self.infer_block(then_branch, environment, expected)?,
                        self.infer_block(else_branch, environment, expected)?,
                    )
                };
                self.merge_structural_join_evidence(&[then_type.clone(), else_type.clone()])?;
                join_types(self.resolve(&then_type), self.resolve(&else_type))
            }
            ExprKind::IfLet {
                pattern,
                value,
                then_branch,
                else_branch,
            } => {
                let value_type = self.infer(value, environment, None)?;
                let resolved_value_type = self.expose_pattern_type(&value_type);
                let analysis = crate::pattern::analyze_pattern(pattern, &value_type);
                if analysis.compatibility == crate::pattern::PatternCompatibility::Incompatible
                    && analysis.problems.is_empty()
                {
                    let location =
                        crate::pattern::first_incompatible_location(pattern, &resolved_value_type)
                            .unwrap_or(pattern.location);
                    self.pattern_diagnostics.entry(location).or_insert_with(|| {
                        format!(
                            "pattern cannot match {}",
                            resolved_value_type.display_name()
                        )
                    });
                }
                for problem in analysis.problems {
                    self.pattern_diagnostics
                        .entry(problem.location)
                        .or_insert(problem.message);
                }
                let mut then_environment = environment.clone();
                self.scheme_scopes.push(HashMap::new());
                for binding in analysis.bindings {
                    self.pattern_binding_types
                        .insert(binding.location, binding.ty.clone());
                    self.set_local_scheme(binding.name.clone(), None);
                    then_environment.insert(binding.name, binding.ty);
                }
                let then_type = self.infer_block(then_branch, &then_environment, expected);
                self.scheme_scopes.pop();
                let then_type = then_type?;
                let else_type = self.infer_block(else_branch, environment, expected)?;
                self.merge_structural_join_evidence(&[then_type.clone(), else_type.clone()])?;
                join_types(self.resolve(&then_type), self.resolve(&else_type))
            }
            ExprKind::LetElse {
                pattern,
                value,
                else_branch,
                body,
            } => {
                let value_type = self.infer(value, environment, None)?;
                let resolved_value_type = self.expose_pattern_type(&value_type);
                let analysis = crate::pattern::analyze_pattern(pattern, &value_type);
                if analysis.irrefutable {
                    self.pattern_diagnostics
                        .entry(pattern.location)
                        .or_insert_with(|| "let else pattern is irrefutable".into());
                }
                if analysis.compatibility == crate::pattern::PatternCompatibility::Incompatible
                    && analysis.problems.is_empty()
                {
                    self.pattern_diagnostics
                        .entry(pattern.location)
                        .or_insert_with(|| {
                            format!(
                                "pattern cannot match {}",
                                resolved_value_type.display_name()
                            )
                        });
                }
                for problem in analysis.problems {
                    self.pattern_diagnostics
                        .entry(problem.location)
                        .or_insert(problem.message);
                }
                let else_type = self.infer_block(else_branch, environment, None)?;
                if !matches!(self.resolve(&else_type), TypeDescriptor::Never) {
                    return Err(format!(
                        "let else branch must have type Never, found {}",
                        self.resolve(&else_type).display_name()
                    ));
                }
                let mut body_environment = environment.clone();
                self.scheme_scopes.push(HashMap::new());
                for binding in analysis.bindings {
                    self.pattern_binding_types
                        .insert(binding.location, binding.ty.clone());
                    self.set_local_scheme(binding.name.clone(), None);
                    body_environment.insert(binding.name, binding.ty);
                }
                let body_type = self.infer_block(body, &body_environment, expected);
                self.scheme_scopes.pop();
                body_type?
            }
            ExprKind::Match { value, arms } => {
                let value_type = self.infer(value, environment, None)?;
                let resolved_value_type = self.expose_pattern_type(&value_type);
                let mut arm_types = Vec::with_capacity(arms.len());
                let mut arm_evidence = Vec::new();
                let mut covered_variants = BTreeSet::new();
                let mut all_values_covered = false;
                for arm in arms {
                    if let Some(query) = &self.query {
                        query.check().map_err(|error| error.to_string())?;
                    }
                    let (mut arm_environment, arm_expected, evidence) = if let Some(expected) =
                        expected
                        && contains_type_variable(&self.resolve(expected))
                    {
                        let (expected, environment, evidence) =
                            self.freshen_join_context(expected, environment);
                        (environment, Some(expected), Some(evidence))
                    } else {
                        (environment.clone(), None, None)
                    };
                    let analysis = crate::pattern::analyze_pattern(&arm.value.pattern, &value_type);
                    if analysis.compatibility == crate::pattern::PatternCompatibility::Incompatible
                        && !arm.value.irrefutable_required
                        && analysis.problems.is_empty()
                    {
                        let location = crate::pattern::first_incompatible_location(
                            &arm.value.pattern,
                            &resolved_value_type,
                        )
                        .unwrap_or(arm.value.pattern.location);
                        self.pattern_diagnostics.entry(location).or_insert_with(|| {
                            format!(
                                "pattern cannot match {}",
                                resolved_value_type.display_name()
                            )
                        });
                    }
                    if arm.value.irrefutable_required && !analysis.irrefutable {
                        let location = crate::pattern::first_refutable_location(
                            &arm.value.pattern,
                            &resolved_value_type,
                        )
                        .unwrap_or(arm.value.pattern.location);
                        self.pattern_diagnostics.entry(location).or_insert_with(|| {
                            format!(
                                "refutable let pattern for {}",
                                resolved_value_type.display_name()
                            )
                        });
                    }
                    let redundant_variants = analysis
                        .possible_variants
                        .iter()
                        .filter(|variant| covered_variants.contains(*variant))
                        .cloned()
                        .collect::<Vec<_>>();
                    let unreachable = all_values_covered
                        || !analysis.possible_variants.is_empty()
                            && redundant_variants.len() == analysis.possible_variants.len();
                    if unreachable {
                        let message = if all_values_covered {
                            "unreachable match arm; prior arms cover every value".to_owned()
                        } else {
                            format!(
                                "unreachable match arm; prior arms cover {}",
                                redundant_variants
                                    .iter()
                                    .map(|variant| format!("'{variant}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        self.pattern_diagnostics
                            .entry(arm.value.pattern.location)
                            .or_insert(message);
                    }
                    if arm.value.guard.is_none() {
                        covered_variants.extend(analysis.covered_variants.iter().cloned());
                        all_values_covered |= analysis.irrefutable;
                        if let TypeDescriptor::Enum(variants) = &resolved_value_type {
                            all_values_covered |= variants
                                .keys()
                                .all(|variant| covered_variants.contains(variant));
                        }
                    }
                    for problem in analysis.problems {
                        self.pattern_diagnostics
                            .entry(problem.location)
                            .or_insert(problem.message);
                    }
                    for duplicate in analysis.duplicates {
                        self.pattern_diagnostics
                            .entry(duplicate.location)
                            .or_insert_with(|| {
                                format!("duplicate pattern binding {:?}", duplicate.name)
                            });
                    }
                    self.scheme_scopes.push(HashMap::new());
                    for binding in analysis.bindings {
                        let binding_type = evidence
                            .as_ref()
                            .map(|replacements| {
                                replace_inference_variables(&binding.ty, replacements)
                            })
                            .unwrap_or(binding.ty);
                        self.pattern_binding_types
                            .insert(binding.location, binding_type.clone());
                        self.set_local_scheme(binding.name.clone(), None);
                        arm_environment.insert(binding.name, binding_type);
                    }
                    if let Some(guard) = &arm.value.guard {
                        self.infer(guard, &arm_environment, Some(&normalized_bool_descriptor()))?;
                    }
                    let arm_type = self.infer(
                        &arm.value.value,
                        &arm_environment,
                        arm_expected.as_ref().or(expected),
                    );
                    self.scheme_scopes.pop();
                    arm_types.push(arm_type?);
                    if let Some(evidence) = evidence {
                        arm_evidence.push(evidence);
                    }
                }
                self.merge_join_evidence(&arm_evidence)?;
                self.merge_structural_join_evidence(&arm_types)?;
                if let TypeDescriptor::Enum(variants) = &resolved_value_type {
                    let missing = variants
                        .iter()
                        .filter(|(name, _)| !covered_variants.contains(*name))
                        .map(|(name, payload)| {
                            if payload.is_some() {
                                format!("'{name}(_)")
                            } else {
                                format!("'{name}")
                            }
                        })
                        .collect::<Vec<_>>();
                    if !missing.is_empty() {
                        self.pattern_diagnostics
                            .entry(expression.location)
                            .or_insert_with(|| {
                                format!("non-exhaustive match; missing {}", missing.join(", "))
                            });
                    }
                }
                if let Some(first) = arm_types.first().cloned() {
                    arm_types
                        .into_iter()
                        .skip(1)
                        .fold(self.resolve(&first), |joined, arm| {
                            join_types(joined, self.resolve(&arm))
                        })
                } else {
                    TypeDescriptor::Any
                }
            }
        };
        if let Some(expected) = expected
            && !(self.recursive_body_inference_depth > 0
                && matches!(expression.value, ExprKind::Closure { .. }))
        {
            self.check(&inferred, expected)?;
        }
        let inferred = self.resolve(&inferred);
        self.records.insert(expression.location, inferred.clone());
        Ok(inferred)
    }

    fn is_builtin_tuple(&self, expression: &Expr) -> bool {
        if !self.builtin_tuple_available {
            return false;
        }
        self.hir
            .expression_ids_at(expression.location)
            .filter_map(|id| self.hir.expression(id))
            .filter_map(|expression| expression.reference)
            .filter_map(|id| self.hir.reference(id))
            .any(|reference| {
                reference.name == "Tuple" && reference.resolution == HirResolution::External
            })
    }

    fn infer_block(
        &mut self,
        block: &Block,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        self.scheme_scopes.push(HashMap::new());
        let result = self.infer_block_scoped(block, environment, expected);
        self.scheme_scopes.pop();
        result
    }

    fn infer_block_scoped(
        &mut self,
        block: &Block,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        let mut environment = environment.clone();
        let mut delayed = Vec::new();
        let mut recursive_skeletons = HashMap::new();
        let component_plan = definition_component_plan(block, self.hir);
        if !component_plan.indirect_recursive.is_empty() {
            return Err("indirect recursive definition requires an explicit contract".into());
        }
        let uncontracted_definition_names = block
            .value
            .bindings
            .iter()
            .filter(|binding| {
                binding.value.kind == BindingKind::Def && binding.value.annotation.is_none()
            })
            .map(|binding| binding.value.name.value.clone())
            .collect::<HashSet<_>>();
        for binding in &block.value.bindings {
            if binding.value.kind != BindingKind::Def || binding.value.annotation.is_some() {
                continue;
            }
            if !component_plan
                .recursive
                .contains(&binding.value.name.location)
            {
                continue;
            }
            let first_owned_variable = self.next_variable;
            if let Some(skeleton) = self.recursive_closure_skeleton(&binding.value.value) {
                environment.insert(binding.value.name.value.clone(), skeleton.clone());
                self.set_local_scheme(binding.value.name.value.clone(), None);
                recursive_skeletons.insert(
                    binding.value.name.value.clone(),
                    (skeleton.clone(), first_owned_variable),
                );
                delayed.push((
                    binding.value.name.value.clone(),
                    skeleton,
                    first_owned_variable,
                ));
            }
        }
        let recursive_variables = recursive_skeletons
            .values()
            .filter_map(|(skeleton, _)| Self::recursive_result_variable(skeleton))
            .collect::<HashSet<_>>();
        for binding in &block.value.bindings {
            let Some((skeleton, _)) = recursive_skeletons.get(&binding.value.name.value) else {
                continue;
            };
            self.delayed_initializer_depth += 1;
            self.recursive_body_inference_depth += 1;
            let recursive_expected = Self::recursive_expected(skeleton);
            let inferred = self.infer(
                &binding.value.value,
                &environment,
                Some(&recursive_expected),
            );
            self.recursive_body_inference_depth -= 1;
            self.delayed_initializer_depth -= 1;
            let inferred = inferred?;
            if let (
                Some(variable),
                TypeDescriptor::Function {
                    result: inferred_result,
                    ..
                },
            ) = (Self::recursive_result_variable(skeleton), inferred)
            {
                self.recursive_equations.insert(variable, *inferred_result);
            }
        }
        self.solve_recursive_equations(&recursive_variables)?;
        for location in &component_plan.acyclic {
            let binding = block
                .value
                .bindings
                .iter()
                .find(|binding| binding.value.name.location == *location)
                .expect("component binding exists");
            let first_owned_variable = self.next_variable;
            self.delayed_initializer_depth += 1;
            let inferred = self.infer(&binding.value.value, &environment, None);
            self.delayed_initializer_depth -= 1;
            let inferred = inferred?;
            let scheme = self.generalize_local_closure(
                &inferred,
                first_owned_variable,
                binding.value.name.location,
            )?;
            let descriptor = scheme
                .as_ref()
                .map_or_else(|| self.resolve(&inferred), |scheme| scheme.body.clone());
            environment.insert(binding.value.name.value.clone(), descriptor);
            self.set_local_scheme(binding.value.name.value.clone(), scheme.clone());
            if let Some(scheme) = scheme {
                self.inferred_schemes
                    .insert(binding.value.name.location, scheme);
            } else {
                delayed.push((
                    binding.value.name.value.clone(),
                    inferred,
                    first_owned_variable,
                ));
            }
        }
        for binding in &block.value.bindings {
            if recursive_skeletons.contains_key(&binding.value.name.value) {
                continue;
            }
            if component_plan
                .acyclic
                .contains(&binding.value.name.location)
            {
                continue;
            }
            let annotated_expected = binding
                .value
                .annotation
                .as_ref()
                .and_then(|annotation| self.local_annotations.get(&annotation.location));
            let binding_expected = annotated_expected.or_else(|| {
                recursive_skeletons
                    .get(&binding.value.name.value)
                    .map(|(skeleton, _)| skeleton)
            });
            let is_recursive = recursive_skeletons.contains_key(&binding.value.name.value);
            if binding.value.kind == BindingKind::Def
                && binding.value.annotation.is_none()
                && !is_recursive
                && expression_references_names(
                    &binding.value.value,
                    &uncontracted_definition_names,
                    &HashSet::new(),
                )
            {
                return Err(format!(
                    "recursive definition {:?} requires a closure value or explicit contract",
                    binding.value.name.value
                ));
            }
            let is_delayed = (annotated_expected.is_none() || is_recursive)
                && matches!(binding.value.kind, BindingKind::Let | BindingKind::Def);
            let first_owned_variable = recursive_skeletons
                .get(&binding.value.name.value)
                .map_or(self.next_variable, |(_, first)| *first);
            if is_delayed {
                self.delayed_initializer_depth += 1;
            }
            let inferred = self.infer(&binding.value.value, &environment, binding_expected);
            if is_delayed {
                self.delayed_initializer_depth -= 1;
            }
            let inferred = inferred?;
            if matches!(
                binding.value.kind,
                BindingKind::Let | BindingKind::Def | BindingKind::Import
            ) {
                let inferred_scheme = if binding.value.kind == BindingKind::Let
                    && binding.value.annotation.is_none()
                    && binding.value.type_parameters.is_empty()
                    && matches!(binding.value.value.value, ExprKind::Closure { .. })
                {
                    self.generalize_local_closure(
                        &inferred,
                        first_owned_variable,
                        binding.value.name.location,
                    )?
                } else {
                    None
                };
                let descriptor = inferred_scheme.as_ref().map_or_else(
                    || binding_expected.cloned().unwrap_or(inferred),
                    |scheme| scheme.body.clone(),
                );
                environment.insert(binding.value.name.value.clone(), descriptor.clone());
                self.set_local_scheme(binding.value.name.value.clone(), inferred_scheme.clone());
                if let Some(scheme) = &inferred_scheme {
                    self.inferred_schemes
                        .insert(binding.value.name.location, scheme.clone());
                }
                if is_delayed && !is_recursive && inferred_scheme.is_none() {
                    delayed.push((
                        binding.value.name.value.clone(),
                        descriptor,
                        first_owned_variable,
                    ));
                }
            }
        }
        let result = self.infer(&block.value.result, &environment, expected)?;
        for (name, descriptor, first_owned_variable) in delayed {
            if let Some(query) = &self.query {
                query.check().map_err(|error| error.to_string())?;
            }
            let resolved = self.resolve(&descriptor);
            if contains_inference_variable_at_or_after(&resolved, first_owned_variable) {
                return Err(format!(
                    "cannot infer monomorphic binding {name:?}: unresolved {}",
                    resolved.display_name()
                ));
            }
        }
        Ok(self.resolve(&result))
    }
}

fn collect_inference_variables(
    descriptor: &TypeDescriptor,
    variables: &mut Vec<InferenceVariableId>,
) {
    match descriptor {
        TypeDescriptor::Inference(variable) => {
            if !variables.contains(variable) {
                variables.push(*variable);
            }
        }
        TypeDescriptor::Array(item)
        | TypeDescriptor::Dict(item)
        | TypeDescriptor::TypeOf(item)
        | TypeDescriptor::Tagged { payload: item, .. } => {
            collect_inference_variables(item, variables);
        }
        TypeDescriptor::Declared(declared) => {
            for argument in declared.id.arguments() {
                collect_inference_variables(argument, variables);
            }
            collect_inference_variables(&declared.body, variables);
        }
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            for item in items {
                collect_inference_variables(item, variables);
            }
        }
        TypeDescriptor::Struct(fields) => {
            for field in fields.values() {
                collect_inference_variables(field, variables);
            }
        }
        TypeDescriptor::Enum(variants) => {
            for payload in variants.values().flatten() {
                collect_inference_variables(payload, variables);
            }
        }
        TypeDescriptor::Function { parameters, result } => {
            for parameter in parameters {
                collect_inference_variables(parameter, variables);
            }
            collect_inference_variables(result, variables);
        }
        TypeDescriptor::Bound(_)
        | TypeDescriptor::Named(_)
        | TypeDescriptor::Any
        | TypeDescriptor::Never
        | TypeDescriptor::Type
        | TypeDescriptor::Dyn
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Atom(_) => {}
    }
}

fn replace_inference_variables(
    descriptor: &TypeDescriptor,
    replacements: &HashMap<InferenceVariableId, InferenceVariableId>,
) -> TypeDescriptor {
    match descriptor {
        TypeDescriptor::Inference(variable) => replacements.get(variable).map_or_else(
            || descriptor.clone(),
            |fresh| TypeDescriptor::Inference(*fresh),
        ),
        TypeDescriptor::Declared(declared) => {
            let arguments = declared
                .id
                .arguments()
                .iter()
                .map(|argument| replace_inference_variables(argument, replacements))
                .collect::<Vec<_>>();
            TypeDescriptor::Declared(DeclaredTypeDescriptor {
                id: declared.id.reapply(&arguments),
                name: declared.name.clone(),
                body: Arc::new(replace_inference_variables(&declared.body, replacements)),
            })
        }
        TypeDescriptor::Array(item) => {
            TypeDescriptor::Array(Box::new(replace_inference_variables(item, replacements)))
        }
        TypeDescriptor::Dict(item) => {
            TypeDescriptor::Dict(Box::new(replace_inference_variables(item, replacements)))
        }
        TypeDescriptor::TypeOf(item) => {
            TypeDescriptor::TypeOf(Box::new(replace_inference_variables(item, replacements)))
        }
        TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
            tag: tag.clone(),
            payload: Box::new(replace_inference_variables(payload, replacements)),
        },
        TypeDescriptor::Tuple(items) => TypeDescriptor::Tuple(
            items
                .iter()
                .map(|item| replace_inference_variables(item, replacements))
                .collect(),
        ),
        TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
            fields
                .iter()
                .map(|(name, field)| {
                    (
                        name.clone(),
                        replace_inference_variables(field, replacements),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Enum(variants) => TypeDescriptor::Enum(
            variants
                .iter()
                .map(|(name, payload)| {
                    (
                        name.clone(),
                        payload.as_ref().map(|payload| {
                            Box::new(replace_inference_variables(payload, replacements))
                        }),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Union(items) => TypeDescriptor::Union(
            items
                .iter()
                .map(|item| replace_inference_variables(item, replacements))
                .collect(),
        ),
        TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
            parameters: parameters
                .iter()
                .map(|parameter| replace_inference_variables(parameter, replacements))
                .collect(),
            result: Box::new(replace_inference_variables(result, replacements)),
        },
        TypeDescriptor::Bound(_)
        | TypeDescriptor::Named(_)
        | TypeDescriptor::Any
        | TypeDescriptor::Never
        | TypeDescriptor::Type
        | TypeDescriptor::Dyn
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Atom(_) => descriptor.clone(),
    }
}

fn rename_named_types(
    descriptor: &TypeDescriptor,
    names: &HashMap<String, String>,
) -> TypeDescriptor {
    match descriptor {
        TypeDescriptor::Named(name) => names
            .get(name)
            .cloned()
            .map(TypeDescriptor::Named)
            .unwrap_or_else(|| descriptor.clone()),
        TypeDescriptor::Declared(declared) => {
            let arguments = declared
                .id
                .arguments()
                .iter()
                .map(|argument| rename_named_types(argument, names))
                .collect::<Vec<_>>();
            TypeDescriptor::Declared(DeclaredTypeDescriptor {
                id: declared.id.reapply(&arguments),
                name: declared.name.clone(),
                body: Arc::new(rename_named_types(&declared.body, names)),
            })
        }
        TypeDescriptor::Array(item) => {
            TypeDescriptor::Array(Box::new(rename_named_types(item, names)))
        }
        TypeDescriptor::Dict(item) => {
            TypeDescriptor::Dict(Box::new(rename_named_types(item, names)))
        }
        TypeDescriptor::TypeOf(item) => {
            TypeDescriptor::TypeOf(Box::new(rename_named_types(item, names)))
        }
        TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
            tag: tag.clone(),
            payload: Box::new(rename_named_types(payload, names)),
        },
        TypeDescriptor::Tuple(items) => TypeDescriptor::Tuple(
            items
                .iter()
                .map(|item| rename_named_types(item, names))
                .collect(),
        ),
        TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), rename_named_types(field, names)))
                .collect(),
        ),
        TypeDescriptor::Enum(variants) => TypeDescriptor::Enum(
            variants
                .iter()
                .map(|(name, payload)| {
                    (
                        name.clone(),
                        payload
                            .as_ref()
                            .map(|payload| Box::new(rename_named_types(payload, names))),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Union(items) => TypeDescriptor::Union(
            items
                .iter()
                .map(|item| rename_named_types(item, names))
                .collect(),
        ),
        TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
            parameters: parameters
                .iter()
                .map(|parameter| rename_named_types(parameter, names))
                .collect(),
            result: Box::new(rename_named_types(result, names)),
        },
        _ => descriptor.clone(),
    }
}

fn normalize_named_names(descriptor: &TypeDescriptor) -> TypeDescriptor {
    let mut names = HashMap::new();
    collect_named_names(descriptor, &mut names);
    rename_named_types(descriptor, &names)
}

fn collect_named_names(descriptor: &TypeDescriptor, names: &mut HashMap<String, String>) {
    match descriptor {
        TypeDescriptor::Named(name) => {
            names.insert(name.clone(), display_named_type(name).to_owned());
        }
        TypeDescriptor::Declared(declared) => {
            for argument in declared.id.arguments() {
                collect_named_names(argument, names);
            }
            collect_named_names(&declared.body, names);
        }
        TypeDescriptor::Array(item)
        | TypeDescriptor::Dict(item)
        | TypeDescriptor::TypeOf(item)
        | TypeDescriptor::Tagged { payload: item, .. } => collect_named_names(item, names),
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            for item in items {
                collect_named_names(item, names);
            }
        }
        TypeDescriptor::Struct(fields) => {
            for field in fields.values() {
                collect_named_names(field, names);
            }
        }
        TypeDescriptor::Enum(variants) => {
            for payload in variants.values().flatten() {
                collect_named_names(payload, names);
            }
        }
        TypeDescriptor::Function { parameters, result } => {
            for parameter in parameters {
                collect_named_names(parameter, names);
            }
            collect_named_names(result, names);
        }
        _ => {}
    }
}

fn collect_bound_parameters(descriptor: &TypeDescriptor, parameters: &mut Vec<TypeParameterId>) {
    match descriptor {
        TypeDescriptor::Bound(parameter) => {
            if !parameters.contains(parameter) {
                parameters.push(*parameter);
            }
        }
        TypeDescriptor::Array(item)
        | TypeDescriptor::Dict(item)
        | TypeDescriptor::TypeOf(item)
        | TypeDescriptor::Tagged { payload: item, .. } => {
            collect_bound_parameters(item, parameters);
        }
        TypeDescriptor::Declared(declared) => {
            for argument in declared.id.arguments() {
                collect_bound_parameters(argument, parameters);
            }
            collect_bound_parameters(&declared.body, parameters);
        }
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            for item in items {
                collect_bound_parameters(item, parameters);
            }
        }
        TypeDescriptor::Struct(fields) => {
            for field in fields.values() {
                collect_bound_parameters(field, parameters);
            }
        }
        TypeDescriptor::Enum(variants) => {
            for payload in variants.values().flatten() {
                collect_bound_parameters(payload, parameters);
            }
        }
        TypeDescriptor::Function {
            parameters: items,
            result,
        } => {
            for item in items {
                collect_bound_parameters(item, parameters);
            }
            collect_bound_parameters(result, parameters);
        }
        TypeDescriptor::Inference(_)
        | TypeDescriptor::Named(_)
        | TypeDescriptor::Any
        | TypeDescriptor::Never
        | TypeDescriptor::Type
        | TypeDescriptor::Dyn
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Atom(_) => {}
    }
}

fn validate_publishable_scheme(scheme: &TypeScheme) -> Result<(), String> {
    if contains_type_variable(&scheme.body) {
        return Err(format!(
            "body contains unresolved {}",
            scheme.body.display_name()
        ));
    }
    let declared = scheme
        .parameters
        .iter()
        .map(|parameter| parameter.id)
        .collect::<HashSet<_>>();
    let mut referenced = Vec::new();
    collect_bound_parameters(&scheme.body, &mut referenced);
    if let Some(parameter) = referenced
        .into_iter()
        .find(|parameter| !declared.contains(parameter))
    {
        return Err(format!(
            "body references unbound parameter T{}",
            parameter.0
        ));
    }
    Ok(())
}

fn bind_inference_variables(
    descriptor: &TypeDescriptor,
    replacements: &HashMap<InferenceVariableId, TypeParameterId>,
) -> TypeDescriptor {
    match descriptor {
        TypeDescriptor::Inference(variable) => replacements.get(variable).map_or_else(
            || descriptor.clone(),
            |parameter| TypeDescriptor::Bound(*parameter),
        ),
        TypeDescriptor::Declared(declared) => {
            let arguments = declared
                .id
                .arguments()
                .iter()
                .map(|argument| bind_inference_variables(argument, replacements))
                .collect::<Vec<_>>();
            TypeDescriptor::Declared(DeclaredTypeDescriptor {
                id: declared.id.reapply(&arguments),
                name: declared.name.clone(),
                body: Arc::new(bind_inference_variables(&declared.body, replacements)),
            })
        }
        TypeDescriptor::Array(item) => {
            TypeDescriptor::Array(Box::new(bind_inference_variables(item, replacements)))
        }
        TypeDescriptor::Dict(item) => {
            TypeDescriptor::Dict(Box::new(bind_inference_variables(item, replacements)))
        }
        TypeDescriptor::TypeOf(item) => {
            TypeDescriptor::TypeOf(Box::new(bind_inference_variables(item, replacements)))
        }
        TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
            tag: tag.clone(),
            payload: Box::new(bind_inference_variables(payload, replacements)),
        },
        TypeDescriptor::Tuple(items) => TypeDescriptor::Tuple(
            items
                .iter()
                .map(|item| bind_inference_variables(item, replacements))
                .collect(),
        ),
        TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), bind_inference_variables(field, replacements)))
                .collect(),
        ),
        TypeDescriptor::Enum(variants) => TypeDescriptor::Enum(
            variants
                .iter()
                .map(|(name, payload)| {
                    (
                        name.clone(),
                        payload.as_ref().map(|payload| {
                            Box::new(bind_inference_variables(payload, replacements))
                        }),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Union(items) => TypeDescriptor::Union(
            items
                .iter()
                .map(|item| bind_inference_variables(item, replacements))
                .collect(),
        ),
        TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
            parameters: parameters
                .iter()
                .map(|parameter| bind_inference_variables(parameter, replacements))
                .collect(),
            result: Box::new(bind_inference_variables(result, replacements)),
        },
        descriptor => descriptor.clone(),
    }
}

fn inferred_type_parameter_name(index: usize) -> String {
    u8::try_from(index)
        .ok()
        .filter(|index| *index < 26)
        .map_or_else(
            || format!("T{index}"),
            |index| char::from(b'A' + index).to_string(),
        )
}

fn contains_type_variable(ty: &TypeDescriptor) -> bool {
    match ty {
        TypeDescriptor::Inference(_) => true,
        TypeDescriptor::Bound(_) => false,
        TypeDescriptor::Declared(declared) => {
            declared.id.arguments().iter().any(contains_type_variable)
                || contains_type_variable(&declared.body)
        }
        TypeDescriptor::Array(item) => contains_type_variable(item),
        TypeDescriptor::Dict(item) => contains_type_variable(item),
        TypeDescriptor::TypeOf(instance) => contains_type_variable(instance),
        TypeDescriptor::Tagged { payload, .. } => contains_type_variable(payload),
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            items.iter().any(contains_type_variable)
        }
        TypeDescriptor::Struct(fields) => fields.values().any(contains_type_variable),
        TypeDescriptor::Enum(variants) => variants
            .values()
            .flatten()
            .any(|payload| contains_type_variable(payload)),
        TypeDescriptor::Function { parameters, result } => {
            parameters.iter().any(contains_type_variable) || contains_type_variable(result)
        }
        _ => false,
    }
}

pub(crate) fn contains_named_type(descriptor: &TypeDescriptor) -> bool {
    match descriptor {
        TypeDescriptor::Named(_) => true,
        TypeDescriptor::Declared(declared) => {
            declared.id.arguments().iter().any(contains_named_type)
                || contains_named_type(&declared.body)
        }
        TypeDescriptor::Array(item)
        | TypeDescriptor::Dict(item)
        | TypeDescriptor::TypeOf(item)
        | TypeDescriptor::Tagged { payload: item, .. } => contains_named_type(item),
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            items.iter().any(contains_named_type)
        }
        TypeDescriptor::Struct(fields) => fields.values().any(contains_named_type),
        TypeDescriptor::Enum(variants) => variants
            .values()
            .flatten()
            .any(|payload| contains_named_type(payload)),
        TypeDescriptor::Function { parameters, result } => {
            parameters.iter().any(contains_named_type) || contains_named_type(result)
        }
        _ => false,
    }
}

fn contains_runtime_never_leaf(descriptor: &TypeDescriptor) -> bool {
    match descriptor {
        TypeDescriptor::Never => true,
        TypeDescriptor::Declared(declared) => contains_runtime_never_leaf(&declared.body),
        TypeDescriptor::Array(item)
        | TypeDescriptor::Dict(item)
        | TypeDescriptor::Tagged { payload: item, .. } => contains_runtime_never_leaf(item),
        TypeDescriptor::Tuple(items) => items.iter().any(contains_runtime_never_leaf),
        TypeDescriptor::Struct(fields) => fields.values().any(contains_runtime_never_leaf),
        TypeDescriptor::Any
        | TypeDescriptor::Named(_)
        | TypeDescriptor::Type
        | TypeDescriptor::TypeOf(_)
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Atom(_)
        | TypeDescriptor::Enum(_)
        | TypeDescriptor::Union(_)
        | TypeDescriptor::Function { .. }
        | TypeDescriptor::Bound(_)
        | TypeDescriptor::Inference(_) => false,
        TypeDescriptor::Dyn => false,
    }
}

fn expression_references_names(
    expression: &Expr,
    names: &HashSet<String>,
    bound: &HashSet<String>,
) -> bool {
    match &expression.value {
        ExprKind::Variable(name) => names.contains(&name.value) && !bound.contains(&name.value),
        ExprKind::InterpolatedString(parts) => parts.iter().any(|part| match &part.value {
            StringPartKind::Text(_) => false,
            StringPartKind::Expression(expression) => {
                expression_references_names(expression, names, bound)
            }
        }),
        ExprKind::Array(items) | ExprKind::Tuple(items) => items
            .iter()
            .any(|item| expression_references_names(item, names, bound)),
        ExprKind::Spread(operand) => expression_references_names(operand, names, bound),
        ExprKind::Dict(fields) => fields
            .iter()
            .any(|field| expression_references_names(&field.value.value, names, bound)),
        ExprKind::Block(block) => {
            let mut block_bound = bound.clone();
            for binding in &block.value.bindings {
                if expression_references_names(&binding.value.value, names, &block_bound) {
                    return true;
                }
                block_bound.insert(binding.value.name.value.clone());
            }
            expression_references_names(&block.value.result, names, &block_bound)
        }
        ExprKind::Unary { operand, .. }
        | ExprKind::Propagate { operand }
        | ExprKind::Field {
            receiver: operand, ..
        }
        | ExprKind::TupleProjection {
            receiver: operand, ..
        } => expression_references_names(operand, names, bound),
        ExprKind::Return { value } => expression_references_names(value, names, bound),
        ExprKind::Panic { message } => expression_references_names(message, names, bound),
        ExprKind::Raise { error } => expression_references_names(error, names, bound),
        ExprKind::Debug { value, .. } => expression_references_names(value, names, bound),
        ExprKind::Binary { left, right, .. } => {
            expression_references_names(left, names, bound)
                || expression_references_names(right, names, bound)
        }
        ExprKind::Index { receiver, index } => {
            expression_references_names(receiver, names, bound)
                || expression_references_names(index, names, bound)
        }
        ExprKind::Call { callee, arguments } => {
            expression_references_names(callee, names, bound)
                || arguments
                    .iter()
                    .any(|argument| expression_references_names(argument, names, bound))
        }
        ExprKind::TypeApply { callee, arguments } => {
            expression_references_names(callee, names, bound)
                || arguments.iter().any(|argument| match &argument.value {
                    TypeArgumentKind::Explicit(argument) => {
                        expression_references_names(argument, names, bound)
                    }
                    TypeArgumentKind::Infer => false,
                })
        }
        ExprKind::Interpreter { operand, .. } => expression_references_names(operand, names, bound),
        ExprKind::Closure {
            parameters,
            result_annotation,
            body,
        } => {
            if parameters.iter().any(|parameter| {
                parameter
                    .annotation
                    .as_ref()
                    .is_some_and(|annotation| expression_references_names(annotation, names, bound))
            }) || result_annotation
                .as_ref()
                .is_some_and(|annotation| expression_references_names(annotation, names, bound))
            {
                return true;
            }
            let mut closure_bound = bound.clone();
            closure_bound.extend(
                parameters
                    .iter()
                    .map(|parameter| parameter.name.value.clone()),
            );
            expression_references_names(
                &located(ExprKind::Block(body.clone()), body.location),
                names,
                &closure_bound,
            )
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            expression_references_names(condition, names, bound)
                || expression_references_names(
                    &located(ExprKind::Block(then_branch.clone()), then_branch.location),
                    names,
                    bound,
                )
                || expression_references_names(
                    &located(ExprKind::Block(else_branch.clone()), else_branch.location),
                    names,
                    bound,
                )
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            expression_references_names(value, names, bound)
                || expression_references_names(&then_branch.value.result, names, bound)
                || expression_references_names(&else_branch.value.result, names, bound)
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            expression_references_names(value, names, bound)
                || expression_references_names(&else_branch.value.result, names, bound)
                || expression_references_names(&body.value.result, names, bound)
        }
        ExprKind::Match { value, arms } => {
            expression_references_names(value, names, bound)
                || arms.iter().any(|arm| {
                    arm.value
                        .guard
                        .as_ref()
                        .is_some_and(|guard| expression_references_names(guard, names, bound))
                        || expression_references_names(&arm.value.value, names, bound)
                })
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_) => false,
    }
}

pub(crate) fn program_references_name(program: &Program, name: &str) -> bool {
    HirProgram::resolve(program, Vec::<String>::new())
        .references()
        .iter()
        .any(|reference| {
            reference.name == name && reference.resolution == HirResolution::Unresolved
        })
}

fn validate_export_references<'a>(
    program: &Program,
    prelude: impl Iterator<Item = &'a String>,
    external_values: &BTreeMap<String, Value>,
    sources: &SourceDatabase,
) -> Result<(), FrontendError> {
    let authored = program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| {
            !matches!(
                binding.value.kind,
                BindingKind::OpenImport | BindingKind::Export
            )
        })
        .map(|binding| binding.value.name.value.as_str())
        .collect::<HashSet<_>>();
    let mut visible = prelude.cloned().collect::<HashSet<_>>();
    visible.extend(
        external_values
            .keys()
            .filter(|name| !authored.contains(name.as_str()))
            .cloned(),
    );
    for binding in &program.value.body.value.bindings {
        if binding.value.kind == BindingKind::Export {
            let local = binding
                .value
                .imported_name
                .as_deref()
                .expect("export markers retain their local name");
            if !visible.contains(&local.value) {
                return Err(FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!("cannot export unknown or forward binding {:?}", local.value),
                        local.location,
                    ),
                ));
            }
        } else if binding.value.kind != BindingKind::OpenImport {
            visible.insert(binding.value.name.value.clone());
        }
    }
    Ok(())
}

pub(crate) fn recovered_reference_locations(
    program: &crate::parser::RecoveredProgram,
    name: &str,
) -> Vec<crate::source::Location> {
    HirProgram::resolve_recovered(program, Vec::<String>::new())
        .references()
        .iter()
        .filter(|reference| {
            reference.name == name && reference.resolution == HirResolution::Unresolved
        })
        .map(|reference| reference.location)
        .collect()
}

fn contains_inference_variable_at_or_after(ty: &TypeDescriptor, first: u32) -> bool {
    match ty {
        TypeDescriptor::Inference(variable) => variable.0 >= first,
        TypeDescriptor::Array(item) | TypeDescriptor::Dict(item) | TypeDescriptor::TypeOf(item) => {
            contains_inference_variable_at_or_after(item, first)
        }
        TypeDescriptor::Tagged { payload, .. } => {
            contains_inference_variable_at_or_after(payload, first)
        }
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => items
            .iter()
            .any(|item| contains_inference_variable_at_or_after(item, first)),
        TypeDescriptor::Struct(fields) => fields
            .values()
            .any(|field| contains_inference_variable_at_or_after(field, first)),
        TypeDescriptor::Enum(variants) => variants
            .values()
            .flatten()
            .any(|payload| contains_inference_variable_at_or_after(payload, first)),
        TypeDescriptor::Function { parameters, result } => {
            parameters
                .iter()
                .any(|parameter| contains_inference_variable_at_or_after(parameter, first))
                || contains_inference_variable_at_or_after(result, first)
        }
        _ => false,
    }
}

fn contains_any_inference_variable(
    ty: &TypeDescriptor,
    variables: &HashSet<InferenceVariableId>,
) -> bool {
    match ty {
        TypeDescriptor::Inference(variable) => variables.contains(variable),
        TypeDescriptor::Array(item) | TypeDescriptor::Dict(item) | TypeDescriptor::TypeOf(item) => {
            contains_any_inference_variable(item, variables)
        }
        TypeDescriptor::Tagged { payload, .. } => {
            contains_any_inference_variable(payload, variables)
        }
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => items
            .iter()
            .any(|item| contains_any_inference_variable(item, variables)),
        TypeDescriptor::Struct(fields) => fields
            .values()
            .any(|field| contains_any_inference_variable(field, variables)),
        TypeDescriptor::Enum(variants) => variants
            .values()
            .flatten()
            .any(|payload| contains_any_inference_variable(payload, variables)),
        TypeDescriptor::Function { parameters, result } => {
            parameters
                .iter()
                .any(|parameter| contains_any_inference_variable(parameter, variables))
                || contains_any_inference_variable(result, variables)
        }
        _ => false,
    }
}

fn contains_metatype(ty: &TypeDescriptor) -> bool {
    match ty {
        TypeDescriptor::Type => true,
        TypeDescriptor::TypeOf(instance) => contains_metatype(instance),
        TypeDescriptor::Array(item) => contains_metatype(item),
        TypeDescriptor::Dict(item) => contains_metatype(item),
        TypeDescriptor::Tagged { payload, .. } => contains_metatype(payload),
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            items.iter().any(contains_metatype)
        }
        TypeDescriptor::Struct(fields) => fields.values().any(contains_metatype),
        TypeDescriptor::Enum(variants) => variants
            .values()
            .flatten()
            .any(|payload| contains_metatype(payload)),
        TypeDescriptor::Function { parameters, result } => {
            parameters.iter().any(contains_metatype) || contains_metatype(result)
        }
        _ => false,
    }
}

fn infer_expr_recorded(
    expression: &Expr,
    environment: &HashMap<String, TypeDescriptor>,
    facts: &mut HashMap<crate::Location, TypeDescriptor>,
) -> TypeDescriptor {
    infer_expr_with(expression, environment, &mut |location, descriptor| {
        facts.insert(location, descriptor.clone());
    })
}

fn infer_expr_with(
    expression: &Expr,
    environment: &HashMap<String, TypeDescriptor>,
    record: &mut impl FnMut(crate::Location, &TypeDescriptor),
) -> TypeDescriptor {
    let inferred = match &expression.value {
        ExprKind::Int(_) => TypeDescriptor::Int,
        ExprKind::Float(_) => TypeDescriptor::Float,
        ExprKind::String(_) => TypeDescriptor::String,
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    infer_expr_with(expression, environment, record);
                }
            }
            TypeDescriptor::String
        }
        ExprKind::Bytes(_) => TypeDescriptor::Bytes,
        ExprKind::Atom(name) => TypeDescriptor::Atom(atom_from_name(name)),
        ExprKind::Variable(name) => environment
            .get(&name.value)
            .cloned()
            .unwrap_or(TypeDescriptor::Any),
        ExprKind::Array(items) => {
            let item_types = items
                .iter()
                .map(|item| {
                    if let ExprKind::Spread(operand) = &item.value {
                        match infer_expr_with(operand, environment, record) {
                            TypeDescriptor::Array(item) => *item,
                            _ => TypeDescriptor::Any,
                        }
                    } else {
                        infer_expr_with(item, environment, record)
                    }
                })
                .collect::<Vec<_>>();
            let item = common_type(item_types).unwrap_or(TypeDescriptor::Any);
            TypeDescriptor::Array(Box::new(item))
        }
        ExprKind::Spread(operand) => infer_expr_with(operand, environment, record),
        ExprKind::Tuple(items) => TypeDescriptor::Tuple(
            items
                .iter()
                .map(|item| infer_expr_with(item, environment, record))
                .collect(),
        ),
        ExprKind::Dict(fields) if fields.iter().any(|field| field.value.name.is_none()) => {
            let items = fields
                .iter()
                .map(|field| {
                    if field.value.name.is_none() {
                        let ExprKind::Spread(operand) = &field.value.value.value else {
                            return TypeDescriptor::Any;
                        };
                        match infer_expr_with(operand, environment, record) {
                            TypeDescriptor::Dict(item) => *item,
                            _ => TypeDescriptor::Any,
                        }
                    } else {
                        infer_expr_with(&field.value.value, environment, record)
                    }
                })
                .collect();
            TypeDescriptor::Dict(Box::new(common_type(items).unwrap_or(TypeDescriptor::Any)))
        }
        ExprKind::Dict(fields) => TypeDescriptor::Struct(
            fields
                .iter()
                .map(|field| {
                    (
                        field
                            .value
                            .name
                            .as_ref()
                            .expect("ordinary Dict field has a name")
                            .value
                            .clone(),
                        infer_expr_with(&field.value.value, environment, record),
                    )
                })
                .collect(),
        ),
        ExprKind::Block(block) => infer_block_with(block, environment, record),
        ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } => {
            infer_expr_with(operand, environment, record)
        }
        ExprKind::Return { value } => {
            infer_expr_with(value, environment, record);
            TypeDescriptor::Never
        }
        ExprKind::Panic { message } => {
            infer_expr_with(message, environment, record);
            TypeDescriptor::Never
        }
        ExprKind::Raise { error } => {
            infer_expr_with(error, environment, record);
            TypeDescriptor::Never
        }
        ExprKind::Debug { value, .. } => infer_expr_with(value, environment, record),
        ExprKind::Binary {
            operator,
            left,
            right,
        } => match operator.value {
            BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEqual
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEqual
            | BinaryOperator::Equal
            | BinaryOperator::NotEqual => TypeDescriptor::Union(vec![
                TypeDescriptor::Atom(Atom::builtin(BuiltinAtom::True)),
                TypeDescriptor::Atom(Atom::builtin(BuiltinAtom::False)),
            ]),
            _ => {
                let left = infer_expr_with(left, environment, record);
                let right = infer_expr_with(right, environment, record);
                if left == right {
                    left
                } else {
                    TypeDescriptor::Any
                }
            }
        },
        ExprKind::Field { receiver, field } => {
            match infer_expr_with(receiver, environment, record) {
                TypeDescriptor::Struct(fields) => fields
                    .get(&field.value)
                    .cloned()
                    .unwrap_or(TypeDescriptor::Any),
                _ => TypeDescriptor::Any,
            }
        }
        ExprKind::Index { receiver, index } => {
            let receiver = infer_expr_with(receiver, environment, record);
            infer_expr_with(index, environment, record);
            match receiver {
                TypeDescriptor::Array(item) => *item,
                _ => TypeDescriptor::Any,
            }
        }
        ExprKind::TupleProjection { receiver, index } => {
            match infer_expr_with(receiver, environment, record) {
                TypeDescriptor::Tuple(items) => items
                    .get(index.value)
                    .cloned()
                    .unwrap_or(TypeDescriptor::Any),
                _ => TypeDescriptor::Any,
            }
        }
        ExprKind::TypeApply { callee, arguments } => {
            infer_expr_with(callee, environment, record);
            for argument in arguments {
                match &argument.value {
                    TypeArgumentKind::Explicit(argument) => {
                        infer_expr_with(argument, environment, record);
                    }
                    TypeArgumentKind::Infer => {
                        record(argument.location, &TypeDescriptor::Any);
                    }
                }
            }
            TypeDescriptor::Any
        }
        ExprKind::Call { callee, arguments } => {
            let callee = infer_expr_with(callee, environment, record);
            let argument_types = arguments
                .iter()
                .map(|argument| infer_expr_with(argument, environment, record))
                .collect::<Vec<_>>();
            match callee {
                TypeDescriptor::Function { result, .. } => *result,
                TypeDescriptor::Atom(tag) if argument_types.len() == 1 => TypeDescriptor::Tagged {
                    tag,
                    payload: Box::new(argument_types.into_iter().next().expect("one argument")),
                },
                _ => TypeDescriptor::Any,
            }
        }
        ExprKind::Interpreter { elaboration, .. } => {
            infer_expr_with(elaboration, environment, record)
        }
        ExprKind::Closure {
            parameters,
            result_annotation,
            body,
        } => {
            for annotation in parameters
                .iter()
                .filter_map(|parameter| parameter.annotation.as_ref())
                .chain(result_annotation.as_deref())
            {
                infer_expr_with(annotation, environment, record);
            }
            let mut closure_environment = environment.clone();
            for parameter in parameters {
                closure_environment.insert(parameter.name.value.clone(), TypeDescriptor::Any);
            }
            TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Any; parameters.len()],
                result: Box::new(infer_block_with(body, &closure_environment, record)),
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            infer_expr_with(condition, environment, record);
            canonical_union(vec![
                infer_block_with(then_branch, environment, record),
                infer_block_with(else_branch, environment, record),
            ])
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            infer_expr_with(value, environment, record);
            join_types(
                infer_block_with(then_branch, environment, record),
                infer_block_with(else_branch, environment, record),
            )
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            infer_expr_with(value, environment, record);
            infer_block_with(else_branch, environment, record);
            infer_block_with(body, environment, record)
        }
        ExprKind::Match { value, arms } => {
            infer_expr_with(value, environment, record);
            canonical_union(
                arms.iter()
                    .map(|arm| {
                        let mut arm_environment = environment.clone();
                        bind_pattern_types(&arm.value.pattern, &mut arm_environment);
                        if let Some(guard) = &arm.value.guard {
                            infer_expr_with(guard, &arm_environment, record);
                        }
                        infer_expr_with(&arm.value.value, &arm_environment, record)
                    })
                    .collect(),
            )
        }
    };
    record(expression.location, &inferred);
    inferred
}

fn check_interpolations(
    expression: &Expr,
    environment: &HashMap<String, TypeDescriptor>,
    sources: &SourceDatabase,
) -> Result<(), FrontendError> {
    match &expression.value {
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(part_expression) = &part.value {
                    let inferred = infer_expr(part_expression, environment);
                    if !interpolation_type_supported(&inferred) {
                        let message = format!(
                            "string interpolation does not support {}",
                            inferred.display_name()
                        );
                        return Err(FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(message, part_expression.location),
                        ));
                    }
                    check_interpolations(part_expression, environment, sources)?;
                }
            }
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                check_interpolations(item, environment, sources)?;
            }
        }
        ExprKind::Spread(operand) => check_interpolations(operand, environment, sources)?,
        ExprKind::Dict(fields) => {
            for field in fields {
                check_interpolations(&field.value.value, environment, sources)?;
            }
        }
        ExprKind::Block(block) => check_block_interpolations(block, environment, sources)?,
        ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } => {
            check_interpolations(operand, environment, sources)?;
        }
        ExprKind::Return { value } => check_interpolations(value, environment, sources)?,
        ExprKind::Panic { message } => check_interpolations(message, environment, sources)?,
        ExprKind::Raise { error } => check_interpolations(error, environment, sources)?,
        ExprKind::Debug { value, .. } => check_interpolations(value, environment, sources)?,
        ExprKind::Binary { left, right, .. } => {
            check_interpolations(left, environment, sources)?;
            check_interpolations(right, environment, sources)?;
        }
        ExprKind::Field { receiver, .. } => {
            check_interpolations(receiver, environment, sources)?;
        }
        ExprKind::Index { receiver, index } => {
            check_interpolations(receiver, environment, sources)?;
            check_interpolations(index, environment, sources)?;
        }
        ExprKind::TupleProjection { receiver, .. } => {
            check_interpolations(receiver, environment, sources)?;
        }
        ExprKind::Call { callee, arguments } => {
            check_interpolations(callee, environment, sources)?;
            for argument in arguments {
                check_interpolations(argument, environment, sources)?;
            }
        }
        ExprKind::TypeApply { callee, arguments } => {
            check_interpolations(callee, environment, sources)?;
            for argument in arguments {
                if let TypeArgumentKind::Explicit(argument) = &argument.value {
                    check_interpolations(argument, environment, sources)?;
                }
            }
        }
        ExprKind::Interpreter { operand, .. } => {
            check_interpolations(operand, environment, sources)?;
        }
        ExprKind::Closure {
            parameters,
            result_annotation,
            body,
        } => {
            for annotation in parameters
                .iter()
                .filter_map(|parameter| parameter.annotation.as_ref())
                .chain(result_annotation.as_deref())
            {
                check_interpolations(annotation, environment, sources)?;
            }
            let mut closure_environment = environment.clone();
            for parameter in parameters {
                closure_environment.insert(parameter.name.value.clone(), TypeDescriptor::Any);
            }
            check_block_interpolations(body, &closure_environment, sources)?;
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            check_interpolations(condition, environment, sources)?;
            check_block_interpolations(then_branch, environment, sources)?;
            check_block_interpolations(else_branch, environment, sources)?;
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            check_interpolations(value, environment, sources)?;
            check_block_interpolations(then_branch, environment, sources)?;
            check_block_interpolations(else_branch, environment, sources)?;
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            check_interpolations(value, environment, sources)?;
            check_block_interpolations(else_branch, environment, sources)?;
            check_block_interpolations(body, environment, sources)?;
        }
        ExprKind::Match { value, arms } => {
            check_interpolations(value, environment, sources)?;
            for arm in arms {
                let mut arm_environment = environment.clone();
                bind_pattern_types(&arm.value.pattern, &mut arm_environment);
                if let Some(guard) = &arm.value.guard {
                    check_interpolations(guard, &arm_environment, sources)?;
                }
                check_interpolations(&arm.value.value, &arm_environment, sources)?;
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_)
        | ExprKind::Variable(_) => {}
    }
    Ok(())
}

fn check_block_interpolations(
    block: &Block,
    environment: &HashMap<String, TypeDescriptor>,
    sources: &SourceDatabase,
) -> Result<(), FrontendError> {
    let mut environment = environment.clone();
    for binding in &block.value.bindings {
        if matches!(binding.value.kind, BindingKind::Decl | BindingKind::Native) {
            environment.insert(binding.value.name.value.clone(), TypeDescriptor::Any);
        }
    }
    for binding in &block.value.bindings {
        check_interpolations(&binding.value.value, &environment, sources)?;
        if let Some(annotation) = &binding.value.annotation {
            check_interpolations(annotation, &environment, sources)?;
        }
        if matches!(
            binding.value.kind,
            BindingKind::Let | BindingKind::Def | BindingKind::Import
        ) {
            let inferred = infer_expr(&binding.value.value, &environment);
            environment.insert(binding.value.name.value.clone(), inferred);
        }
    }
    check_interpolations(&block.value.result, &environment, sources)
}

fn interpolation_type_supported(descriptor: &TypeDescriptor) -> bool {
    match descriptor {
        TypeDescriptor::Bound(_) | TypeDescriptor::Inference(_) | TypeDescriptor::Named(_) => false,
        TypeDescriptor::Declared(declared) => interpolation_type_supported(&declared.body),
        TypeDescriptor::Any
        | TypeDescriptor::Never
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Atom(_) => true,
        TypeDescriptor::Union(variants) => variants.iter().all(interpolation_type_supported),
        TypeDescriptor::Enum(variants) => variants.iter().all(|(name, payload)| {
            interpolation_type_supported(&enum_variant_type(name, payload.as_deref()))
        }),
        TypeDescriptor::Type
        | TypeDescriptor::Dyn
        | TypeDescriptor::TypeOf(_)
        | TypeDescriptor::Bytes
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Array(_)
        | TypeDescriptor::Dict(_)
        | TypeDescriptor::Tagged { .. }
        | TypeDescriptor::Tuple(_)
        | TypeDescriptor::Struct(_)
        | TypeDescriptor::Function { .. } => false,
    }
}

fn infer_block_with(
    block: &Block,
    environment: &HashMap<String, TypeDescriptor>,
    record: &mut impl FnMut(crate::Location, &TypeDescriptor),
) -> TypeDescriptor {
    let mut environment = environment.clone();
    for binding in &block.value.bindings {
        if matches!(binding.value.kind, BindingKind::Decl | BindingKind::Native) {
            environment.insert(binding.value.name.value.clone(), TypeDescriptor::Any);
        }
    }
    for binding in &block.value.bindings {
        if let Some(annotation) = &binding.value.annotation {
            infer_expr_with(annotation, &environment, record);
        }
        let inferred = infer_expr_with(&binding.value.value, &environment, record);
        if matches!(
            binding.value.kind,
            BindingKind::Let | BindingKind::Def | BindingKind::Import
        ) {
            environment.insert(binding.value.name.value.clone(), inferred);
        }
    }
    infer_expr_with(&block.value.result, &environment, record)
}

fn bind_pattern_types(pattern: &Pattern, environment: &mut HashMap<String, TypeDescriptor>) {
    bind_pattern_types_from(pattern, &TypeDescriptor::Any, environment);
}

fn bind_pattern_types_from(
    pattern: &Pattern,
    matched: &TypeDescriptor,
    environment: &mut HashMap<String, TypeDescriptor>,
) {
    for binding in crate::pattern::analyze_pattern(pattern, matched).bindings {
        environment.insert(binding.name, binding.ty);
    }
}

fn common_type(types: Vec<TypeDescriptor>) -> Option<TypeDescriptor> {
    let first = types.first()?.clone();
    if types.iter().all(|item| item == &first) {
        Some(first)
    } else if types
        .iter()
        .all(|item| assignable(item, &TypeDescriptor::Type))
    {
        Some(TypeDescriptor::Type)
    } else {
        None
    }
}

fn substitute_bound_parameters(
    descriptor: &TypeDescriptor,
    replacements: &HashMap<TypeParameterId, TypeDescriptor>,
) -> TypeDescriptor {
    match descriptor {
        TypeDescriptor::Bound(parameter) => replacements
            .get(parameter)
            .cloned()
            .unwrap_or_else(|| descriptor.clone()),
        TypeDescriptor::Declared(declared) => {
            let body = substitute_bound_parameters(&declared.body, replacements);
            let arguments = declared
                .id
                .arguments()
                .iter()
                .map(|argument| substitute_bound_parameters(argument, replacements))
                .collect::<Vec<_>>();
            TypeDescriptor::Declared(DeclaredTypeDescriptor {
                id: declared.id.reapply(&arguments),
                name: declared.name.clone(),
                body: Arc::new(body),
            })
        }
        TypeDescriptor::Array(item) => {
            TypeDescriptor::Array(Box::new(substitute_bound_parameters(item, replacements)))
        }
        TypeDescriptor::Dict(item) => {
            TypeDescriptor::Dict(Box::new(substitute_bound_parameters(item, replacements)))
        }
        TypeDescriptor::TypeOf(item) => {
            TypeDescriptor::TypeOf(Box::new(substitute_bound_parameters(item, replacements)))
        }
        TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
            tag: tag.clone(),
            payload: Box::new(substitute_bound_parameters(payload, replacements)),
        },
        TypeDescriptor::Tuple(items) => TypeDescriptor::Tuple(
            items
                .iter()
                .map(|item| substitute_bound_parameters(item, replacements))
                .collect(),
        ),
        TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
            fields
                .iter()
                .map(|(name, field)| {
                    (
                        name.clone(),
                        substitute_bound_parameters(field, replacements),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Enum(variants) => TypeDescriptor::Enum(
            variants
                .iter()
                .map(|(name, payload)| {
                    (
                        name.clone(),
                        payload.as_ref().map(|payload| {
                            Box::new(substitute_bound_parameters(payload, replacements))
                        }),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Union(variants) => canonical_union(
            variants
                .iter()
                .map(|variant| substitute_bound_parameters(variant, replacements))
                .collect(),
        ),
        TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
            parameters: parameters
                .iter()
                .map(|parameter| substitute_bound_parameters(parameter, replacements))
                .collect(),
            result: Box::new(substitute_bound_parameters(result, replacements)),
        },
        _ => descriptor.clone(),
    }
}

pub(crate) fn apply_declared_type_arguments(
    id: &crate::value::DeclaredTypeId,
    arguments: &[TypeDescriptor],
) -> crate::value::DeclaredTypeId {
    let replacements = arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            (
                TypeParameterId(u32::try_from(index).expect("type family arity exceeds u32")),
                argument.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let applied = id
        .arguments()
        .iter()
        .map(|argument| substitute_bound_parameters(argument, &replacements))
        .collect::<Vec<_>>();
    id.reapply(&applied)
}

fn erase_type_variables(descriptor: &TypeDescriptor) -> TypeDescriptor {
    match descriptor {
        TypeDescriptor::Bound(_) | TypeDescriptor::Inference(_) => TypeDescriptor::Any,
        TypeDescriptor::Declared(declared) => {
            let arguments = declared
                .id
                .arguments()
                .iter()
                .map(erase_type_variables)
                .collect::<Vec<_>>();
            TypeDescriptor::Declared(DeclaredTypeDescriptor {
                id: declared.id.reapply(&arguments),
                name: declared.name.clone(),
                body: Arc::new(erase_type_variables(&declared.body)),
            })
        }
        TypeDescriptor::Array(item) => TypeDescriptor::Array(Box::new(erase_type_variables(item))),
        TypeDescriptor::Dict(item) => TypeDescriptor::Dict(Box::new(erase_type_variables(item))),
        TypeDescriptor::TypeOf(instance) => {
            TypeDescriptor::TypeOf(Box::new(erase_type_variables(instance)))
        }
        TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
            tag: tag.clone(),
            payload: Box::new(erase_type_variables(payload)),
        },
        TypeDescriptor::Tuple(items) => {
            TypeDescriptor::Tuple(items.iter().map(erase_type_variables).collect())
        }
        TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), erase_type_variables(field)))
                .collect(),
        ),
        TypeDescriptor::Enum(variants) => TypeDescriptor::Enum(
            variants
                .iter()
                .map(|(name, payload)| {
                    (
                        name.clone(),
                        payload
                            .as_ref()
                            .map(|payload| Box::new(erase_type_variables(payload))),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Union(variants) => {
            TypeDescriptor::Union(variants.iter().map(erase_type_variables).collect())
        }
        TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
            parameters: parameters.iter().map(erase_type_variables).collect(),
            result: Box::new(erase_type_variables(result)),
        },
        descriptor => descriptor.clone(),
    }
}

fn join_all_types(types: Vec<TypeDescriptor>) -> TypeDescriptor {
    types.into_iter().fold(TypeDescriptor::Never, join_types)
}

fn potentially_assignable(actual: &TypeDescriptor, expected: &TypeDescriptor) -> bool {
    if matches!(actual, TypeDescriptor::Inference(_) | TypeDescriptor::Any)
        || matches!(expected, TypeDescriptor::Inference(_) | TypeDescriptor::Any)
    {
        return true;
    }
    match (actual, expected) {
        (TypeDescriptor::Array(actual), TypeDescriptor::Array(expected))
        | (TypeDescriptor::Dict(actual), TypeDescriptor::Dict(expected))
        | (TypeDescriptor::TypeOf(actual), TypeDescriptor::TypeOf(expected)) => {
            potentially_assignable(actual, expected)
        }
        (TypeDescriptor::Tuple(actual), TypeDescriptor::Tuple(expected))
            if actual.len() == expected.len() =>
        {
            actual
                .iter()
                .zip(expected)
                .all(|(actual, expected)| potentially_assignable(actual, expected))
        }
        (TypeDescriptor::Struct(actual), TypeDescriptor::Struct(expected))
            if actual.keys().eq(expected.keys()) =>
        {
            actual
                .iter()
                .all(|(name, actual)| potentially_assignable(actual, &expected[name]))
        }
        _ => assignable(actual, expected),
    }
}

fn join_types(left: TypeDescriptor, right: TypeDescriptor) -> TypeDescriptor {
    if left == right {
        return left;
    }
    if matches!(left, TypeDescriptor::Never) {
        return right;
    }
    if matches!(right, TypeDescriptor::Never) {
        return left;
    }
    if matches!(left, TypeDescriptor::Any) || matches!(right, TypeDescriptor::Any) {
        return TypeDescriptor::Any;
    }
    if assignable(&left, &TypeDescriptor::Type) && assignable(&right, &TypeDescriptor::Type) {
        return TypeDescriptor::Type;
    }
    let left_to_right = assignable(&left, &right);
    let right_to_left = assignable(&right, &left);
    match (left_to_right, right_to_left) {
        (true, false) => right,
        (false, true) => left,
        _ => canonical_union(vec![left, right]),
    }
}

fn canonical_union(types: Vec<TypeDescriptor>) -> TypeDescriptor {
    fn flatten(ty: TypeDescriptor, flattened: &mut Vec<TypeDescriptor>) {
        match ty {
            TypeDescriptor::Union(variants) => {
                for variant in variants {
                    flatten(variant, flattened);
                }
            }
            ty => flattened.push(ty),
        }
    }

    let mut flattened = Vec::new();
    for ty in types {
        flatten(ty, &mut flattened);
    }
    if flattened.iter().any(|ty| matches!(ty, TypeDescriptor::Any)) {
        return TypeDescriptor::Any;
    }
    if flattened
        .iter()
        .any(|ty| !matches!(ty, TypeDescriptor::Never))
    {
        flattened.retain(|ty| !matches!(ty, TypeDescriptor::Never));
    }
    flattened.sort_by_cached_key(|ty| (ty.display_name(), format!("{ty:?}")));
    flattened.dedup();
    match flattened.len() {
        0 => TypeDescriptor::Never,
        1 => flattened.pop().expect("one canonical Union member"),
        _ => TypeDescriptor::Union(flattened),
    }
}

pub(crate) fn assignable(actual: &TypeDescriptor, expected: &TypeDescriptor) -> bool {
    match (actual, expected) {
        (TypeDescriptor::Never, _) => true,
        (TypeDescriptor::Any, _) | (_, TypeDescriptor::Any) => true,
        (TypeDescriptor::TypeOf(_), TypeDescriptor::Type) => true,
        (TypeDescriptor::TypeOf(actual), TypeDescriptor::TypeOf(expected)) => {
            assignable(actual, expected)
        }
        (TypeDescriptor::Declared(actual), TypeDescriptor::Declared(expected)) => {
            actual.id == expected.id
        }
        (TypeDescriptor::Enum(actual), TypeDescriptor::Enum(expected)) => {
            actual.len() == expected.len()
                && expected.iter().all(|(name, expected)| {
                    actual
                        .get(name)
                        .is_some_and(|actual| match (actual, expected) {
                            (None, None) => true,
                            (Some(actual), Some(expected)) => assignable(actual, expected),
                            _ => false,
                        })
                })
        }
        (TypeDescriptor::Union(variants), expected @ TypeDescriptor::Enum(_)) => {
            variants.iter().all(|variant| assignable(variant, expected))
        }
        (actual, TypeDescriptor::Enum(variants)) => variants.iter().any(|(name, payload)| {
            assignable(actual, &enum_variant_type(name, payload.as_deref()))
        }),
        (TypeDescriptor::Enum(variants), expected) => variants.iter().all(|(name, payload)| {
            assignable(&enum_variant_type(name, payload.as_deref()), expected)
        }),
        (TypeDescriptor::Union(actual), TypeDescriptor::Union(expected)) => actual
            .iter()
            .all(|actual| expected.iter().any(|expected| assignable(actual, expected))),
        (actual, TypeDescriptor::Union(variants)) => {
            variants.iter().any(|variant| assignable(actual, variant))
        }
        (TypeDescriptor::Union(variants), expected) => {
            variants.iter().all(|variant| assignable(variant, expected))
        }
        (TypeDescriptor::Array(actual), TypeDescriptor::Array(expected)) => {
            assignable(actual, expected)
        }
        (TypeDescriptor::Dict(actual), TypeDescriptor::Dict(expected)) => {
            assignable(actual, expected)
        }
        (TypeDescriptor::Struct(actual), TypeDescriptor::Dict(expected)) => {
            actual.values().all(|actual| assignable(actual, expected))
        }
        (
            TypeDescriptor::Tagged {
                tag: actual_tag,
                payload: actual,
            },
            TypeDescriptor::Tagged {
                tag: expected_tag,
                payload: expected,
            },
        ) => actual_tag == expected_tag && assignable(actual, expected),
        (TypeDescriptor::Tuple(actual), TypeDescriptor::Tuple(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| assignable(actual, expected))
        }
        (TypeDescriptor::Struct(actual), TypeDescriptor::Struct(expected)) => {
            actual.len() == expected.len()
                && expected.iter().all(|(name, expected)| {
                    actual
                        .get(name)
                        .is_some_and(|actual| assignable(actual, expected))
                })
        }
        (
            TypeDescriptor::Function {
                parameters: actual_parameters,
                result: actual_result,
            },
            TypeDescriptor::Function {
                parameters: expected_parameters,
                result: expected_result,
            },
        ) => {
            actual_parameters.len() == expected_parameters.len()
                && actual_parameters
                    .iter()
                    .zip(expected_parameters)
                    .all(|(actual, expected)| assignable(actual, expected))
                && assignable(actual_result, expected_result)
        }
        _ => actual == expected,
    }
}

pub(crate) fn erase_declared_identity(descriptor: &TypeDescriptor) -> TypeDescriptor {
    match descriptor {
        TypeDescriptor::Declared(declared) => erase_declared_identity(&declared.body),
        TypeDescriptor::Array(item) => {
            TypeDescriptor::Array(Box::new(erase_declared_identity(item)))
        }
        TypeDescriptor::Dict(item) => TypeDescriptor::Dict(Box::new(erase_declared_identity(item))),
        TypeDescriptor::TypeOf(instance) => {
            TypeDescriptor::TypeOf(Box::new(erase_declared_identity(instance)))
        }
        TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
            tag: tag.clone(),
            payload: Box::new(erase_declared_identity(payload)),
        },
        TypeDescriptor::Tuple(items) => {
            TypeDescriptor::Tuple(items.iter().map(erase_declared_identity).collect())
        }
        TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), erase_declared_identity(field)))
                .collect(),
        ),
        TypeDescriptor::Enum(variants) => TypeDescriptor::Enum(
            variants
                .iter()
                .map(|(name, payload)| {
                    (
                        name.clone(),
                        payload
                            .as_ref()
                            .map(|payload| Box::new(erase_declared_identity(payload))),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Union(variants) => {
            TypeDescriptor::Union(variants.iter().map(erase_declared_identity).collect())
        }
        TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
            parameters: parameters.iter().map(erase_declared_identity).collect(),
            result: Box::new(erase_declared_identity(result)),
        },
        descriptor => descriptor.clone(),
    }
}

fn enum_variant_type(name: &str, payload: Option<&TypeDescriptor>) -> TypeDescriptor {
    let tag = TypeDescriptor::Atom(atom_from_name(name));
    payload.map_or(tag, |payload| TypeDescriptor::Tagged {
        tag: atom_from_name(name),
        payload: Box::new(payload.clone()),
    })
}

fn incompatibility_path(actual: &TypeDescriptor, expected: &TypeDescriptor) -> Option<ValuePath> {
    fn visit(actual: &TypeDescriptor, expected: &TypeDescriptor, path: &mut ValuePath) -> bool {
        match (actual, expected) {
            (TypeDescriptor::Any, _) | (_, TypeDescriptor::Any) => false,
            (TypeDescriptor::Struct(actual), TypeDescriptor::Struct(expected)) => {
                for (name, expected) in expected {
                    path.push(ValuePathSegment::Key(name.clone()));
                    let mismatch = actual
                        .get(name)
                        .is_none_or(|actual| visit(actual, expected, path));
                    if mismatch {
                        return true;
                    }
                    path.pop();
                }
                if let Some(name) = actual.keys().find(|name| !expected.contains_key(*name)) {
                    path.push(ValuePathSegment::Key(name.clone()));
                    return true;
                }
                false
            }
            (TypeDescriptor::Enum(actual), TypeDescriptor::Enum(expected)) => {
                for (name, expected) in expected {
                    path.push(ValuePathSegment::Key(name.clone()));
                    let mismatch = match (actual.get(name), expected) {
                        (Some(None), None) => false,
                        (Some(Some(actual)), Some(expected)) => visit(actual, expected, path),
                        _ => true,
                    };
                    if mismatch {
                        return true;
                    }
                    path.pop();
                }
                if let Some(name) = actual.keys().find(|name| !expected.contains_key(*name)) {
                    path.push(ValuePathSegment::Key(name.clone()));
                    return true;
                }
                false
            }
            (TypeDescriptor::Tuple(actual), TypeDescriptor::Tuple(expected)) => {
                if actual.len() != expected.len() {
                    return true;
                }
                for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                    path.push(ValuePathSegment::Index(index));
                    if visit(actual, expected, path) {
                        return true;
                    }
                    path.pop();
                }
                false
            }
            (TypeDescriptor::Array(actual), TypeDescriptor::Array(expected)) => {
                visit(actual, expected, path)
            }
            (TypeDescriptor::Dict(actual), TypeDescriptor::Dict(expected)) => {
                visit(actual, expected, path)
            }
            (TypeDescriptor::Struct(actual), TypeDescriptor::Dict(expected)) => {
                for (name, actual) in actual {
                    path.push(ValuePathSegment::Key(name.clone()));
                    if visit(actual, expected, path) {
                        return true;
                    }
                    path.pop();
                }
                false
            }
            (
                TypeDescriptor::Tagged {
                    tag: actual_tag,
                    payload: actual,
                },
                TypeDescriptor::Tagged {
                    tag: expected_tag,
                    payload: expected,
                },
            ) => actual_tag != expected_tag || visit(actual, expected, path),
            _ => !assignable(actual, expected),
        }
    }
    let mut path = Vec::new();
    visit(actual, expected, &mut path).then_some(path)
}

fn expression_location_at_path(
    expression: &Expr,
    path: &[ValuePathSegment],
) -> Option<crate::Location> {
    let mut expression = expression;
    for segment in path {
        expression = match (segment, &expression.value) {
            (ValuePathSegment::Key(name), ExprKind::Dict(fields)) => fields
                .iter()
                .find(|field| {
                    field
                        .value
                        .name
                        .as_ref()
                        .is_some_and(|field_name| field_name.value == *name)
                })
                .map(|field| &field.value.value)?,
            (ValuePathSegment::Index(index), ExprKind::Array(items))
            | (ValuePathSegment::Index(index), ExprKind::Tuple(items)) => items.get(*index)?,
            _ => return None,
        };
    }
    Some(expression.location)
}

fn atom_from_name(name: &str) -> Atom {
    match name {
        "None" => Atom::builtin(BuiltinAtom::None),
        "Some" => Atom::builtin(BuiltinAtom::Some),
        "Ok" => Atom::builtin(BuiltinAtom::Ok),
        "Err" => Atom::builtin(BuiltinAtom::Err),
        "True" => Atom::builtin(BuiltinAtom::True),
        "False" => Atom::builtin(BuiltinAtom::False),
        _ => Atom::named(name),
    }
}

fn frontend_error(source_name: &str, message: impl Into<String>) -> FrontendError {
    FrontendError::new(
        source_name,
        SourceLocation {
            offset: 0,
            line: 1,
            column: 1,
        },
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate_legacy_value(descriptor: &TypeDescriptor, value: &Value) -> Result<(), String> {
        crate::vm::with_legacy_value_ref(value, |value| {
            validate_value_ref(descriptor, value, "value")
        })
    }

    #[test]
    fn declared_validation_trusts_matching_owners_but_not_raw_values() {
        let body =
            TypeDescriptor::Struct(BTreeMap::from([("value".to_owned(), TypeDescriptor::Int)]));
        let mut vm = Vm::new();
        let owner = crate::DeclaredType::bind("test", 1, "Number", body.to_value(&mut vm));
        let expected = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: owner.id().clone(),
            name: owner.name().to_owned(),
            body: Arc::new(body.clone()),
        });
        let invalid_payload = vm
            .make_dict([("value".to_owned(), Value::string("not an Int"))])
            .unwrap();
        let trusted = Value::Declared(crate::DeclaredValue::new(
            owner.clone(),
            invalid_payload.clone(),
        ));
        assert_eq!(validate_legacy_value(&expected, &trusted), Ok(()));

        let raw_error = validate_legacy_value(&expected, &invalid_payload).unwrap_err();
        assert!(raw_error.contains("value.value must be Int"), "{raw_error}");

        let other_owner =
            crate::DeclaredType::bind("test", 2, "OtherNumber", body.to_value(&mut vm));
        let other = Value::Declared(crate::DeclaredValue::new(other_owner, invalid_payload));
        let identity_error = validate_legacy_value(&expected, &other).unwrap_err();
        assert!(
            identity_error.contains("different declared type identity"),
            "{identity_error}"
        );

        let valid_raw = vm.make_dict([("value".to_owned(), Value::Int(3))]).unwrap();
        assert_eq!(validate_legacy_value(&expected, &valid_raw), Ok(()));
    }

    #[test]
    fn bootstrap_prelude_keeps_public_projections_consistent() {
        let mut vm = Vm::new();
        let prelude = BootstrapPrelude::new(&mut vm);
        assert!(prelude.values.keys().any(|name| name.starts_with('\0')));
        assert!(prelude.values.contains_key("\0telora_pack_dyn"));
        for name in prelude.schemes.keys() {
            assert!(
                prelude.values.contains_key(name),
                "missing value for {name}"
            );
            assert!(prelude.types.contains_key(name), "missing type for {name}");
        }
    }

    fn analyze_with_natives(
        source: &str,
        natives: &[(&'static str, usize)],
    ) -> Result<Analysis, FrontendError> {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("generic-native.telora", source);
        let parsed = parse_registered(&sources, source_id);
        let program = parsed.program.unwrap_or_else(|| {
            panic!(
                "generic native source parses: {source:?}: {:?}",
                parsed.diagnostics
            )
        });
        let external_values = natives
            .iter()
            .map(|(name, arity)| {
                (
                    (*name).to_owned(),
                    Value::Func(Arc::new(Closure::native(NativeFunction::new(
                        name,
                        *arity,
                        native_validate,
                    )))),
                )
            })
            .collect();
        analyze_program_with_bindings(
            "generic-native.telora",
            &program,
            &mut QuotaAccount::new(Quota::with_fuel(100_000)),
            &external_values,
            &HashSet::new(),
            &sources,
            &BTreeMap::new(),
        )
    }

    fn analyze_with_host_binding(
        source: &str,
        value: Value,
        dynamic: bool,
        interface: Option<TypeScheme>,
    ) -> Result<Analysis, FrontendError> {
        let mut sources = SourceDatabase::default();
        let source_id = sources.add("host-binding.telora", source);
        let parsed = parse_registered(&sources, source_id);
        let program = parsed.program.unwrap_or_else(|| {
            panic!(
                "host binding source parses: {source:?}: {:?}",
                parsed.diagnostics
            )
        });
        let external_values = BTreeMap::from([("host".to_owned(), value)]);
        let dynamic_bindings = if dynamic {
            HashSet::from(["host".to_owned()])
        } else {
            HashSet::new()
        };
        let external_interfaces = interface
            .map(|scheme| {
                BTreeMap::from([(
                    "host".to_owned(),
                    ModuleInterface {
                        exports: BTreeMap::from([("host".to_owned(), scheme)]),
                        concrete_types: BTreeMap::new(),
                        type_family_templates: BTreeMap::new(),
                    },
                )])
            })
            .unwrap_or_default();
        let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
        analyze_program_with_bindings_observed(
            "host-binding.telora",
            &program,
            &mut QuotaAccount::new(Quota::with_fuel(100_000)),
            &external_values,
            &dynamic_bindings,
            &sources,
            &BTreeMap::new(),
            &external_interfaces,
            &debug_sink,
        )
    }

    #[test]
    fn host_bindings_distinguish_erased_dynamic_and_declared_interfaces() {
        let function = || {
            Value::Func(Arc::new(Closure::native(NativeFunction::new(
                "host",
                1,
                native_validate,
            ))))
        };

        let erased = analyze_with_host_binding("host(1)", function(), false, None).unwrap();
        assert_eq!(
            erased.display(erased.binding_types["host"]),
            "Fn(Any) -> Any"
        );
        assert_eq!(erased.display(erased.result_type), "Any");

        let mut interface_sources = SourceDatabase::default();
        let interface_source = interface_sources.add("host-interface", "");
        let interface_location = crate::Location::from_usize(interface_source, 0..0).unwrap();
        let parameter = TypeParameterId(37);
        let declared = analyze_with_host_binding(
            "host(1)",
            function(),
            false,
            Some(TypeScheme {
                parameters: vec![TypeParameter {
                    id: parameter,
                    name: "Value".into(),
                    location: interface_location,
                }],
                body: TypeDescriptor::Function {
                    parameters: vec![TypeDescriptor::Bound(parameter)],
                    result: Box::new(TypeDescriptor::Bound(parameter)),
                },
            }),
        )
        .unwrap();
        assert_eq!(declared.display(declared.result_type), "Int");
        assert_eq!(
            declared.module_interface.exports.get("host"),
            None,
            "a consumed Host interface is not implicitly re-exported"
        );

        let dynamic = analyze_with_host_binding("host", Value::Int(1), true, None).unwrap();
        assert_eq!(dynamic.display(dynamic.binding_types["host"]), "Any");
        assert_eq!(dynamic.display(dynamic.result_type), "Any");

        let chained =
            analyze_with_natives("if 'False { 1 } else if 'True { \"x\" } else { 2.0 }", &[])
                .unwrap();
        let explicit_nested = analyze_with_natives(
            "if 'False { 1 } else { if 'True { \"x\" } else { 2.0 } }",
            &[],
        )
        .unwrap();
        assert_eq!(
            chained.display(chained.result_type),
            explicit_nested.display(explicit_nested.result_type)
        );
    }

    #[test]
    fn unresolved_source_names_fail_before_generic_inference_fallbacks() {
        let error = analyze_with_natives("missing(1)", &[]).unwrap_err();
        assert_eq!(error.message, "unknown binding \"missing\"");
    }

    #[test]
    fn strict_collection_joins_preserve_unions_without_synthesizing_any() {
        let arrays = analyze_with_natives(
            "native stop: Fn() -> Never;\
             let values = [1, \"x\"];\
             let reachable = [stop(), 1];\
             let dynamic: Any = 1;\
             let erased = [dynamic, \"x\"];\
             (values, reachable, erased)",
            &[("stop", 0)],
        )
        .unwrap();
        assert_eq!(
            arrays.display(arrays.binding_types["values"]),
            "Array<Int | String>"
        );
        assert_eq!(
            arrays.display(arrays.binding_types["reachable"]),
            "Array<Int>"
        );
        assert_eq!(arrays.display(arrays.binding_types["erased"]), "Array<Any>");

        let dict = analyze_with_natives(
            "let ints: Dict(Int) = {a: 1};\
             let strings: Dict(String) = {b: \"x\"};\
             let values = {...ints, ...strings};\
             values",
            &[],
        )
        .unwrap();
        assert_eq!(
            dict.display(dict.binding_types["values"]),
            "Dict<Int | String>"
        );

        for source in [
            "let values = [1, \"x\"]; let output: Array(Int) = values; output",
            "let ints: Dict(Int) = {a: 1};\
             let strings: Dict(String) = {b: \"x\"};\
             let values = {...ints, ...strings};\
             let output: Dict(Int) = values; output",
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(
                error.message.contains("String") && error.message.contains("Int"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn strict_field_projection_is_precise_or_diagnostic() {
        let source = "let record = {value: 1};\
                      let dictionary: Dict(String) = {value: \"x\"};\
                      let alternative = if 'True { {value: 1} } else { {value: \"x\"} };\
                      let dynamic: Any = record;\
                      export let output = (record.value, dictionary.value, alternative.value, dynamic.value);";
        let analysis = analyze_with_natives(source, &[]).unwrap();
        assert_eq!(
            analysis.display(analysis.binding_types["output"]),
            "(Int, String, Int | String, Any)"
        );
        assert_eq!(
            analysis.module_interface.exports["output"].display_name(),
            "(Int, String, Int | String, Any)"
        );
        let dictionary_field_start = source.find("dictionary.value").unwrap();
        let dictionary_field = analysis
            .hir
            .expressions()
            .iter()
            .filter(|expression| expression.location.range().start == dictionary_field_start)
            .max_by_key(|expression| expression.location.range().end)
            .expect("dictionary field expression");
        assert_eq!(
            analysis.display(analysis.expression_types[&dictionary_field.id]),
            "String"
        );

        let inferred_accessor = analyze_with_natives(
            "let get = fn(value) { value.name }; get({name: \"x\", extra: 1})",
            &[],
        )
        .unwrap();
        assert_eq!(
            inferred_accessor.display(inferred_accessor.result_type),
            "String"
        );

        let unconstrained =
            analyze_with_natives("let get = fn(value) { value.name }; get", &[]).unwrap_err();
        assert!(
            unconstrained
                .message
                .contains("cannot infer monomorphic binding \"get\""),
            "{}",
            unconstrained.message
        );

        let deferred = analyze_with_natives(
            "native combine: for(Record, Left, Right)\
                 Fn(Fn(Record) -> Left, Fn(Record) -> Right, Record) -> Tuple([Left, Right]);\
             combine(fn(value) { value.left }, fn(value) { value.right }, {left: 1, right: \"x\"})",
            &[("combine", 3)],
        )
        .unwrap();
        assert_eq!(deferred.display(deferred.result_type), "(Int, String)");

        let shadowed = analyze_with_natives(
            "type Holder = struct {value: Int};\
             let value = fn(input) { input };\
             let read: Fn(Holder) -> Int = fn(value) { value.value };\
             read({value: 1})",
            &[],
        )
        .unwrap();
        assert_eq!(shadowed.display(shadowed.result_type), "Int");

        for (source, expected) in [
            (
                "let value = {present: 1}; value.missing",
                "Struct has no field \"missing\"",
            ),
            ("1.missing", "cannot access field \"missing\" on Int"),
            (
                "let value: Dict(String) = {present: \"x\"};\
                 let output: Int = value.present; output",
                "cannot unify String with Int",
            ),
            (
                "let get: Fn(Dyn) -> Any = fn(value) { value.missing }; get",
                "cannot access field \"missing\" on Dyn",
            ),
            (
                "let value = if 'True { {present: 1} } else { {other: 2} };\
                 value.present",
                "Struct has no field \"present\"",
            ),
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(error.message.contains(expected), "{}", error.message);
        }
    }

    #[test]
    fn generic_native_calls_instantiate_fresh_types_and_check_callbacks() {
        let analysis = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A;\
             native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);\
             (identity(1), identity(\"x\"), map([1, 2], fn(x) { x + 1 }))",
            &[("identity", 1), ("map", 2)],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(Int, String, Array<Int>)"
        );
        let identity = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "identity")
            .expect("identity definition");
        assert_eq!(identity.type_parameters[0].name, "A");
        let callback_parameter = analysis
            .hir
            .expressions()
            .iter()
            .find(|expression| {
                expression
                    .reference
                    .and_then(|reference| analysis.hir.reference(reference))
                    .is_some_and(|reference| reference.name == "x")
            })
            .expect("callback parameter expression");
        assert_eq!(
            analysis.display(analysis.expression_types[&callback_parameter.id]),
            "Int"
        );
    }

    #[test]
    fn generic_definition_contracts_check_rigidly_and_instantiate_at_each_use() {
        let analysis = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { value };\
             decl apply: for(A, B) Fn(Fn(A) -> B, A) -> B;\
             def apply = fn(function, value) { function(value) };\
             (identity(1), identity(\"x\"), apply(fn(value) { value + 1 }, 2))",
            &[],
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "(Int, String, Int)");
        assert!(analysis.module_interface.exports.is_empty());

        let invalid = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { 1 };\
             identity",
            &[],
        )
        .unwrap_err();
        assert!(
            invalid.message.contains("cannot unify Int with T0"),
            "{}",
            invalid.message
        );
    }

    #[test]
    fn definition_contracts_evaluate_referenced_concrete_types_first() {
        for (name, source) in [
            (
                "earlier-contract-type.telora",
                "type Plan = struct {name: String};\
                 def builder: Fn(Int) -> Plan = fn(value) { {name: `plan \\{value}`} };\
                 builder(1)",
            ),
            (
                "later-contract-type.telora",
                "def builder: Fn(Int) -> Plan = fn(value) { {name: `plan \\{value}`} };\
                 type Plan = struct {name: String};\
                 builder(1)",
            ),
            (
                "parameter-and-result-contract-types.telora",
                "type Input = struct {value: Int};\
                 type Output = struct {text: String};\
                 def convert: Fn(Input) -> Output = fn(input) { {text: `value \\{input.value}`} };\
                 convert({value: 1})",
            ),
            (
                "transitive-contract-types.telora",
                "type Plan = NamedPlan;\
                 type NamedPlan = struct {name: String};\
                 def builder: Fn(Int) -> Plan = fn(value) { {name: `plan \\{value}`} };\
                 builder(1)",
            ),
        ] {
            let analysis = analyze_source(name, source).unwrap();
            let expected = if name == "parameter-and-result-contract-types.telora" {
                "Output"
            } else if name == "transitive-contract-types.telora" {
                "NamedPlan"
            } else {
                "Plan"
            };
            assert_eq!(analysis.display(analysis.result_type), expected, "{name}");
        }
    }

    #[test]
    fn contracted_definitions_preserve_generic_callback_result_precision() {
        let source = "type Box(T) = struct {value: T};\
                      type Plan = struct {name: String};\
                      def invoke: for(P) Fn(Fn(Int) -> P) -> Box(P) = fn(build) {\
                          {value: build(1)}\
                      };\
                      def builder: Fn(Int) -> Plan = fn(value) {\
                          {name: `plan:\\{value}`}\
                      };\
                      let result = invoke(builder);\
                      let output = result;\
                      {output}";
        let analysis = analyze_source("callback-contract.telora", source).unwrap();
        assert_eq!(analysis.display(analysis.result_type), "{output: Box}");
        assert_eq!(analysis.display(analysis.binding_types["output"]), "Box");

        let with_unused_family = analyze_source(
            "callback-contract-unused-family.telora",
            "type Box(T) = struct {value: T};\
             type Plan = struct {name: String};\
             type Unused(T) = Tuple([Plan, T]);\
             def invoke: for(P) Fn(Fn(Int) -> P) -> Box(P) = fn(build) {\
                 {value: build(1)}\
             };\
             def builder: Fn(Int) -> Plan = fn(value) {\
                 {name: `plan:\\{value}`}\
             };\
             let result = invoke(builder);\
             let output = result;\
             {output}",
        )
        .unwrap();
        assert_eq!(
            with_unused_family.display(with_unused_family.result_type),
            analysis.display(analysis.result_type)
        );
    }

    #[test]
    fn contract_reachable_concrete_type_cycles_are_diagnosed_deterministically() {
        for (name, source, participants) in [
            (
                "direct",
                "type Loop = Loop;\
                 def use: Fn(Loop) -> Int = fn(value) { 0 };\
                 use",
                &["Loop"][..],
            ),
            (
                "mutual",
                "type Left = Right;\
                 type Right = Left;\
                 def use: Fn(Left) -> Int = fn(value) { 0 };\
                 use",
                &["Left", "Right"][..],
            ),
        ] {
            let error =
                analyze_source(&format!("contract-type-{name}-cycle.telora"), source).unwrap_err();
            assert!(
                error
                    .message
                    .contains("recursive type component required by a definition contract"),
                "{error}"
            );
            for participant in participants {
                assert!(error.message.contains(participant), "{error}");
            }
        }

        let recursive = analyze_source(
            "recursive-contract-type.telora",
            "type Node = struct {value: Int, children: Array(Node)};\
             def leaf: Fn(Int) -> Node = fn(value) { {value, children: []} };\
             leaf(1)",
        )
        .expect("decorated recursive type contracts retain the sealing path");
        assert!(recursive.declared_types.contains_key("Node"));
    }

    #[test]
    fn recursive_concrete_types_remain_strict_in_definition_contracts_and_families() {
        let recursive = analyze_source(
            "recursive-node.telora",
            "type Node = struct {value: Int, children: Array(Node)};\
             def value_of: Fn(Node) -> Int = fn(node) { node.value };\
             let node: Node = {value: 1, children: []};\
             value_of(node)",
        )
        .unwrap();
        assert_eq!(recursive.display(recursive.result_type), "Int");
        assert_eq!(
            recursive.display(recursive.binding_types["value_of"]),
            "Fn(Node) -> Int"
        );

        let invalid = analyze_source(
            "recursive-node-invalid.telora",
            "type Node = struct {value: Int, children: Array(Node)};\
             def value_of: Fn(Node) -> Int = fn(node) { node.value };\
             value_of(\"bad\")",
        )
        .unwrap_err();
        assert!(invalid.message.contains("String") && invalid.message.contains("Node"));

        let mutual = analyze_source(
            "recursive-expr.telora",
            "type Expr = enum {'Value(Int), 'Call(CallExpr)};\
             type CallExpr = struct {name: String, args: Array(Expr)};\
             type Renderer(Context) = struct {render: Fn(Context, Expr) -> String};\
             type Context = struct {prefix: String};\
             def inspect: Fn(Expr) -> Int = fn(expr) {\
                 match expr {'Value(value) => value, 'Call(call) => 0}\
             };\
             let expr: Expr = 'Call({name: \"sum\", args: ['Value(1)]});\
             inspect(expr)",
        )
        .unwrap();
        assert_eq!(mutual.display(mutual.result_type), "Int");
        assert!(
            !mutual
                .display(mutual.binding_types["inspect"])
                .contains("Any")
        );
        let renderer = mutual
            .definition_schemes
            .iter()
            .find_map(|(definition, scheme)| {
                (mutual.hir.definition(*definition)?.name == "Renderer").then_some(scheme)
            })
            .expect("Renderer family scheme");
        assert!(!renderer.display_name().contains("Any"));
    }

    #[test]
    fn nested_inference_errors_retain_the_offending_expression_location() {
        let source = "def apply: for(A) Fn(Fn(A, Int, A) -> A, A) -> A = fn(step, acc) {\
                      step(acc, 1)\
                      };\
                      apply(fn(acc, value, extra) { acc + value + extra }, 0)";
        let error = analyze_with_natives(source, &[]).unwrap_err();
        assert!(
            error.message.contains("call expects 3 arguments, found 2"),
            "{}",
            error.message
        );
        let diagnostic = error.diagnostic.expect("located inference diagnostic");
        let call = "step(acc, 1)";
        let start = source.find(call).expect("call expression exists");
        assert_eq!(
            diagnostic.labels[0].location.range(),
            start..start + call.len()
        );
    }

    #[test]
    fn strict_contracts_authorize_related_generic_results_after_shallow_inference() {
        let natives = &[("map", 2), ("flat_map", 2)];
        let helpers = "native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);\
                       native flat_map: for(A, B) Fn(Array(A), Fn(A) -> Array(B)) -> Array(B);\
                       def option_to_list: for(A) Fn(Option(A)) -> Array(A) = fn(value) {\
                           match value { 'Some(item) => [item], 'None => [] }\
                       };\
                       def completed_values: for(A) Fn(Array(Option(A))) -> Array(A) = fn(results) {\
                           flat_map(results, option_to_list)\
                       };";

        let tuple_source = [
            helpers,
            "type Batch(A) = Tuple([Array(Option(A)), Array(A)]);\
             export def collect: for(Input, Output)\
                 Fn(Array(Input), Fn(Input) -> Option(Output)) -> Batch(Output) =\
                 fn(inputs, lower) {\
                     let results = map(inputs, lower);\
                     let values = completed_values(results);\
                     (results, values)\
                 };",
        ]
        .concat();
        let tuple = analyze_with_natives(&tuple_source, natives).unwrap();
        assert_eq!(
            tuple.module_interface.exports["collect"].display_name(),
            "for(Input, Output) Fn(Array<Input>, Fn(Input) -> enum {None, Some(Output)}) -> (Array<enum {None, Some(Output)}>, Array<Output>)"
        );

        let struct_source = [
            helpers,
            "type Batch(A) = struct {\
                 complete: Option(Array(A)),\
                 results: Array(Option(A)),\
                 values: Array(A),\
             };\
             export def collect: for(Input, Output)\
                 Fn(Array(Input), Fn(Input) -> Option(Output)) -> Batch(Output) =\
                 fn(inputs, lower) {\
                     let results = map(inputs, lower);\
                     let values = completed_values(results);\
                     let complete = if 'True { 'Some(values) } else { 'None };\
                     {complete: complete, results: results, values: values}\
                 };",
        ]
        .concat();
        let structure = analyze_with_natives(&struct_source, natives).unwrap();
        assert!(
            structure.module_interface.exports["collect"]
                .display_name()
                .contains("-> Batch")
        );

        let invalid_source = [
            helpers,
            "type Batch(A) = struct {results: Array(Option(A)), values: Array(A)};\
             export def collect: for(Input, Output)\
                 Fn(Array(Input), Fn(Input) -> Option(Output)) -> Batch(Output) =\
                 fn(inputs, lower) {\
                     let results = map(inputs, lower);\
                     {results: results, values: [1]}\
                 };",
        ]
        .concat();
        let invalid = analyze_with_natives(&invalid_source, natives).unwrap_err();
        assert!(
            invalid.message.contains("cannot unify Int with T1"),
            "{}",
            invalid.message
        );
    }

    #[test]
    fn generic_definition_aliases_instantiate_once_and_exports_retain_schemes() {
        let alias_error = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { value };\
             let local = identity;\
             (local(1), local(\"x\"))",
            &[],
        )
        .unwrap_err();
        assert!(alias_error.message.contains("cannot unify String with Int"));

        let exported = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity = fn(value) { value };\
             {identity: identity}",
            &[],
        )
        .unwrap();
        let scheme = &exported.module_interface.exports["identity"];
        assert_eq!(scheme.parameters[0].name, "A");
        assert!(matches!(
            &scheme.body,
            TypeDescriptor::Function { parameters, result }
                if parameters == &[TypeDescriptor::Bound(TypeParameterId(0))]
                    && **result == TypeDescriptor::Bound(TypeParameterId(0))
        ));
        let identity = exported
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "identity")
            .unwrap();
        assert_eq!(
            exported.definition_schemes[&identity.id].display_name(),
            "for(A) Fn(A) -> A"
        );
    }

    #[test]
    fn annotated_definitions_are_atomic_generic_contracts() {
        let analysis = analyze_with_natives(
            "def identity: for(A) Fn(A) -> A = fn(value) { value };\
             (identity(1), identity(\"x\"))",
            &[],
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "(Int, String)");

        let duplicate = analyze_with_natives(
            "decl identity: for(A) Fn(A) -> A;\
             def identity: for(A) Fn(A) -> A = fn(value) { value };\
             identity",
            &[],
        )
        .unwrap_err();
        assert!(
            duplicate
                .message
                .contains("duplicate declaration \"identity\"")
        );

        let specialized = analyze_with_natives(
            "def identity: for(A) Fn(A) -> A = fn(value) { 1 }; identity",
            &[],
        )
        .unwrap_err();
        assert!(
            specialized.message.contains("cannot unify Int with T0"),
            "{}",
            specialized.message
        );
    }

    #[test]
    fn generic_native_result_uses_expected_type_and_rejects_missing_or_conflicting_evidence() {
        let inferred = analyze_with_natives(
            "native empty: for(A) Fn() -> Array(A);\
             let values: Array(Int) = empty();\
             values",
            &[("empty", 0)],
        )
        .unwrap();
        assert_eq!(inferred.display(inferred.result_type), "Array<Int>");

        let missing = analyze_with_natives(
            "native empty: for(A) Fn() -> Array(A); empty()",
            &[("empty", 0)],
        )
        .unwrap_err();
        assert!(missing.message.contains("cannot infer generic result type"));

        let conflicting = analyze_with_natives(
            "native choose: for(A) Fn(A, A) -> A; choose(1, \"x\")",
            &[("choose", 2)],
        )
        .unwrap_err();
        assert!(
            conflicting.message.contains("cannot unify String with Int"),
            "{}",
            conflicting.message
        );
    }

    #[test]
    fn generic_calls_complete_from_the_whole_call_context() {
        for source in [
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A; choose(empty(), 1)",
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A; choose(1, empty())",
            "native stop: Fn() -> Never; native choose: for(A) Fn(A, A) -> A; choose(stop(), 1)",
            "native stop: Fn() -> Never; native choose: for(A) Fn(A, A) -> A; choose(1, stop())",
        ] {
            let analysis =
                analyze_with_natives(source, &[("empty", 0), ("choose", 2), ("stop", 0)]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "Int");
        }

        let callback = analyze_with_natives(
            "native make: for(A, B) Fn(Fn(A) -> B) -> B;\
             let values: Array(Int) = make(fn(value) { [value] }); values",
            &[("make", 1)],
        )
        .unwrap();
        assert_eq!(callback.display(callback.result_type), "Array<Int>");

        let partial = analyze_with_natives(
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A;\
             choose@[_](empty(), \"value\")",
            &[("empty", 0), ("choose", 2)],
        )
        .unwrap();
        assert_eq!(partial.display(partial.result_type), "String");
    }

    #[test]
    fn generic_use_refines_option_result_of_a_let_bound_callback() {
        let analysis = analyze_with_natives(
            "def apply: for(A, B) Fn(A, Fn(A) -> Option(B)) -> Option(B) =\
                 fn(value, f) { f(value) };\
             let build = fn(value) {\
                 if value > 0 { 'Some(\"ok\") } else { 'None }\
             };\
             let unrelated = 1;\
             apply(1, build)",
            &[],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "enum {None, Some(String)}"
        );

        for callback in [
            "fn(value) { if value > 0 { 'Some(1) } else { 'None } }",
            "fn(value) { if value > 0 { 'Some(\"ok\") } else { 'Foreign } }",
        ] {
            let error = analyze_with_natives(
                &format!(
                    "def apply: for(A) Fn(A, Fn(A) -> Option(String)) -> Option(String) =\
                         fn(value, f) {{ f(value) }};\
                     let build = {callback}; apply(1, build)"
                ),
                &[],
            )
            .unwrap_err();
            assert!(
                error.message.contains("Int")
                    || error.message.contains("Foreign")
                    || error.message.contains("Some(String)"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn generic_calls_combine_singleton_atoms_with_closed_enum_evidence() {
        let prelude = "type NodeId = enum {'Base, 'Other};\
             let nodes: Array(NodeId) = ['Other];";
        for call in [
            "def choose: for(Node) Fn(Node, Array(Node)) -> Node = fn(base, nodes) { base };\
             choose('Base, nodes)",
            "def choose: for(Node) Fn(Array(Node), Node) -> Node = fn(nodes, base) { base };\
             choose(nodes, 'Base)",
            "def choose: for(Node) Fn(Node, Node, Array(Node)) -> Node = fn(base, other, nodes) { base };\
             choose('Base, 'Other, nodes)",
        ] {
            let analysis = analyze_with_natives(&format!("{prelude}{call}"), &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "NodeId");
        }

        let conflict = analyze_with_natives(
            "type NodeId = enum {'Base, 'Other};\
             type ForeignId = enum {'Foreign};\
             def choose: for(Node) Fn(Node, Array(Node)) -> Node = fn(base, nodes) { base };\
             let foreign: Array(ForeignId) = ['Foreign];\
             choose('Base, foreign)",
            &[],
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("cannot unify"),
            "{}",
            conflict.message
        );
    }

    #[test]
    fn generic_call_context_rejects_conflicts_and_remains_underconstrained() {
        let conflict = analyze_with_natives(
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A;\
             let value: String = choose(empty(), 1); value",
            &[("empty", 0), ("choose", 2)],
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("cannot unify"),
            "{}",
            conflict.message
        );

        let unresolved = analyze_with_natives(
            "native empty: for(A) Fn() -> A; native choose: for(A) Fn(A, A) -> A;\
             choose(empty(), empty())",
            &[("empty", 0), ("choose", 2)],
        )
        .unwrap_err();
        assert!(
            unresolved
                .message
                .contains("cannot infer generic result type"),
            "{}",
            unresolved.message
        );
    }

    #[test]
    fn never_checks_directionally_without_constraining_generic_evidence() {
        let inferred = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native choose: for(A) Fn(A, A) -> A;\
             (choose(stop(), 1), choose(1, stop()))",
            &[("stop", 0), ("choose", 2)],
        )
        .unwrap();
        assert_eq!(inferred.display(inferred.result_type), "(Int, Int)");

        let missing = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native choose: for(A) Fn(A, A) -> A;\
             choose(stop(), stop())",
            &[("stop", 0), ("choose", 2)],
        )
        .unwrap_err();
        assert!(missing.message.contains("cannot infer generic result type"));

        let expected = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native choose: for(A) Fn(A, A) -> A;\
             let value: String = choose(stop(), stop()); value",
            &[("stop", 0), ("choose", 2)],
        )
        .unwrap();
        assert_eq!(expected.display(expected.result_type), "String");
    }

    #[test]
    fn adversarial_never_evidence_is_directional_through_structures_and_callbacks() {
        for (name, source) in [
            (
                "never-first",
                "native stop: Fn() -> Never; native choose: for(A) Fn(A, A) -> A;\
                 choose([stop()], [1])",
            ),
            (
                "never-last",
                "native stop: Fn() -> Never; native choose: for(A) Fn(A, A) -> A;\
                 choose([1], [stop()])",
            ),
        ] {
            let analysis = analyze_with_natives(source, &[("stop", 0), ("choose", 2)])
                .unwrap_or_else(|error| panic!("{name}: {}", error.message));
            assert_eq!(analysis.display(analysis.result_type), "Array<Int>");
        }

        let callback = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native apply: for(A, B) Fn(A, Fn(A) -> B, B) -> B;\
             apply(1, fn(value) { stop() }, \"fallback\")",
            &[("stop", 0), ("apply", 3)],
        )
        .unwrap();
        assert_eq!(callback.display(callback.result_type), "String");

        let reverse = analyze_with_natives(
            "native produce: for(A) Fn() -> A; let impossible: Never = produce(); impossible",
            &[("produce", 0)],
        )
        .unwrap();
        assert_eq!(reverse.display(reverse.result_type), "Never");
    }

    #[test]
    fn never_is_bottom_for_expected_types_and_branch_results() {
        let analysis = analyze_with_natives(
            "native stop: Fn() -> Never;\
             let value: Int = stop();\
             let branch = if 'True { 1 } else { stop() };\
             let all_never = if 'True { stop() } else { stop() };\
             (value, branch, all_never, Never)",
            &[("stop", 0)],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(Int, Int, Never, TypeOf(Never))"
        );

        let reverse = analyze_with_natives(
            "native produce: Fn() -> Int;\
             let impossible: Never = produce(); impossible",
            &[("produce", 0)],
        )
        .unwrap_err();
        assert!(reverse.message.contains("Int") && reverse.message.contains("Never"));
    }

    #[test]
    fn nested_structural_expectations_preserve_generic_constraints() {
        let inferred = analyze_with_natives(
            "native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             (concat([[1], [], [2]]), concat([[], [1]]), concat([[1], []]))",
            &[("concat", 1)],
        )
        .unwrap();
        assert_eq!(
            inferred.display(inferred.result_type),
            "(Array<Int>, Array<Int>, Array<Int>)"
        );

        let expected = analyze_with_natives(
            "native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             let values: Array(String) = concat([[], []]); values",
            &[("concat", 1)],
        )
        .unwrap();
        assert_eq!(expected.display(expected.result_type), "Array<String>");

        let missing = analyze_with_natives(
            "native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             concat([[], []])",
            &[("concat", 1)],
        )
        .unwrap_err();
        assert!(missing.message.contains("cannot infer generic result type"));
    }

    #[test]
    fn structural_constraints_ignore_never_and_preserve_metadata_widening() {
        let analysis = analyze_with_natives(
            "native stop: Fn() -> Never;\
             native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             native identity: for(A) Fn(Array(A)) -> Array(A);\
             (concat([[stop()], [1]]), identity([Int, String]))",
            &[("stop", 0), ("concat", 1), ("identity", 1)],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(Array<Int>, Array<Type>)"
        );

        let conflict = analyze_with_natives(
            "native concat: for(A) Fn(Array(Array(A))) -> Array(A);\
             concat([[1], [\"x\"]])",
            &[("concat", 1)],
        )
        .unwrap_err();
        assert!(conflict.message.contains("String") && conflict.message.contains("Int"));
    }

    #[test]
    fn unannotated_closures_infer_parameters_from_their_bodies() {
        let arithmetic =
            analyze_with_natives("let increment = fn(value) { value + 1 }; increment", &[])
                .unwrap();
        assert_eq!(arithmetic.display(arithmetic.result_type), "Fn(Int) -> Int");

        let known_call = analyze_with_natives(
            "native length: Fn(String) -> Int;\
             let measure = fn(value) { length(value) }; measure",
            &[("length", 1)],
        )
        .unwrap();
        assert_eq!(
            known_call.display(known_call.result_type),
            "Fn(String) -> Int"
        );

        let related = analyze_with_natives(
            "let combine = fn(left, right) { left + right + 1 }; combine",
            &[],
        )
        .unwrap();
        assert_eq!(related.display(related.result_type), "Fn(Int, Int) -> Int");
    }

    #[test]
    fn unknown_callee_calls_infer_closed_function_shapes() {
        let apply = analyze_with_natives(
            "let apply = fn(callback, value) { callback(value) }; apply",
            &[],
        )
        .unwrap();
        let apply_definition = apply
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "apply")
            .unwrap();
        assert_eq!(
            apply.definition_schemes[&apply_definition.id].display_name(),
            "for(A, B) Fn(Fn(A) -> B, A) -> B"
        );

        let called = analyze_with_natives(
            "let apply = fn(callback, value) { callback(value) };\
             apply(fn(value) { value + 1 }, 41)",
            &[],
        )
        .unwrap();
        assert_eq!(called.display(called.result_type), "Int");

        let intrinsic = analyze_with_natives(
            "let use = fn(callback) { callback(1.0) + 2.0 };\
             use(fn(value) { value })",
            &[],
        )
        .unwrap();
        assert_eq!(intrinsic.display(intrinsic.result_type), "Float");
    }

    #[test]
    fn unknown_callee_calls_use_expected_results_and_existing_completion() {
        let expected = analyze_with_natives(
            "let recover = fn(callback) { callback() };\
             let value: String = recover(fn() { \"ok\" }); value",
            &[],
        )
        .unwrap();
        assert_eq!(expected.display(expected.result_type), "String");

        let conflict = analyze_with_natives(
            "let apply = fn(callback) { callback(1) };\
             apply(fn(value: String) { value })",
            &[],
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("cannot unify"),
            "{}",
            conflict.message
        );

        let incomplete =
            analyze_with_natives("let invoke = fn(callback) { callback() }; invoke", &[]).unwrap();
        let invoke = incomplete
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "invoke")
            .unwrap();
        assert_eq!(
            incomplete.definition_schemes[&invoke.id].display_name(),
            "for(A) Fn(Fn() -> A) -> A"
        );
    }

    #[test]
    fn inferred_callable_obligations_converge_across_repeated_calls() {
        for source in [
            "let use = fn(callback) { (callback(1), callback(2)) };\
             use(fn(value) { value + 1 })",
            "let use = fn(callback) { (callback(2), callback(1)) };\
             use(fn(value) { value + 1 })",
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "(Int, Int)");
        }

        for source in [
            "let use = fn(callback) { (callback(1), callback(\"x\")) }; use",
            "let use = fn(callback) { (callback(\"x\"), callback(1)) }; use",
            "let use = fn(callback) { let alias = callback;\
                 (alias(1), callback(\"x\")) }; use",
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(error.message.contains("cannot unify"), "{}", error.message);
        }

        let arity = analyze_with_natives(
            "let use = fn(callback) { (callback(1), callback(1, 2)) }; use",
            &[],
        )
        .unwrap_err();
        assert!(arity.message.contains("call expects 1 arguments, found 2"));
    }

    #[test]
    fn inferred_callable_obligations_converge_through_nested_calls() {
        let compose = analyze_with_natives(
            "let compose = fn(outer, inner, value) { outer(inner(value)) }; compose",
            &[],
        )
        .unwrap();
        let definition = compose
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "compose")
            .unwrap();
        assert_eq!(
            compose.definition_schemes[&definition.id].display_name(),
            "for(A, B, C) Fn(Fn(A) -> B, Fn(C) -> A, C) -> B"
        );

        let nested = analyze_with_natives(
            "let invoke_factory = fn(factory) { factory()() }; invoke_factory",
            &[],
        )
        .unwrap();
        let definition = nested
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "invoke_factory")
            .unwrap();
        assert_eq!(
            nested.definition_schemes[&definition.id].display_name(),
            "for(A) Fn(Fn() -> Fn() -> A) -> A"
        );

        let executed = analyze_with_natives(
            "let compose = fn(outer, inner, value) { outer(inner(value)) };\
             compose(fn(value) { `value=\\{value}` }, fn(value) { value + 1 }, 41)",
            &[],
        )
        .unwrap();
        assert_eq!(executed.display(executed.result_type), "String");
    }

    #[test]
    fn callable_diagnostics_distinguish_static_values_from_explicit_any() {
        for (source, expected) in [
            ("let value = 1; value(2)", "Int"),
            ("let value = \"text\"; value(1)", "String"),
            ("let value = [1]; value(2)", "Array<Int>"),
            ("let value = {item: 1}; value(2)", "{item: Int}"),
            ("let value = Int; value(1)", "TypeOf(Int)"),
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert_eq!(
                error.message,
                format!("cannot call value of type {expected}")
            );
        }

        let dynamic =
            analyze_with_natives("let callable: Any = fn(value) { value }; callable(1)", &[])
                .unwrap();
        assert_eq!(dynamic.display(dynamic.result_type), "Any");

        let arity = analyze_with_natives(
            "let invoke = fn(callback) { (callback(1), callback(1, 2)) }; invoke",
            &[],
        )
        .unwrap_err();
        assert!(arity.message.contains("call expects 1 arguments, found 2"));
    }

    #[test]
    fn inferred_callable_schemes_publish_separately_from_call_instances() {
        let source = "let apply = fn(callback, value) { callback(value) };\
                      let result = apply(fn(value) { value + 1 }, 41);\
                      {apply: apply, result: result}";
        let analysis = analyze_with_natives(source, &[]).unwrap();
        assert_eq!(
            analysis.module_interface.exports["apply"].display_name(),
            "for(A, B) Fn(Fn(A) -> B, A) -> B"
        );
        assert_eq!(analysis.display(analysis.binding_types["result"]), "Int");

        let call_start = source.find("apply(fn").unwrap();
        let call = analysis
            .hir
            .expressions()
            .iter()
            .filter(|expression| expression.location.range().start == call_start)
            .max_by_key(|expression| expression.location.range().end)
            .unwrap();
        assert_eq!(analysis.display(analysis.expression_types[&call.id]), "Int");
    }

    #[test]
    fn intrinsic_expression_constraints_infer_booleans_and_numeric_families() {
        let condition = analyze_with_natives(
            "let select = fn(condition, value) {\
                 if condition { value } else { value }\
             }; (select('True, 1), select('False, \"value\"))",
            &[],
        )
        .unwrap();
        assert_eq!(condition.display(condition.result_type), "(Int, String)");

        for (source, expected) in [
            ("let add = fn(value) { value + 1 }; add", "Fn(Int) -> Int"),
            ("let add = fn(value) { 1 + value }; add", "Fn(Int) -> Int"),
            (
                "let scale = fn(value) { value * 1.5 }; scale",
                "Fn(Float) -> Float",
            ),
            ("let negative: Float = -1.5; negative", "Float"),
            (
                "let before = fn(value) { value < 1 }; before",
                "Fn(Int) -> enum {False, True}",
            ),
            (
                "let before = fn(value) { value < \"z\" }; before",
                "Fn(String) -> enum {False, True}",
            ),
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), expected);
        }

        let equality = analyze_with_natives("1 == \"1\"", &[]).unwrap();
        assert_eq!(equality.display(equality.result_type), "enum {False, True}");

        let logical_not = analyze_with_natives(
            "let invert_bool: Fn(Bool) -> Bool = fn(value) { !value };\
             let invert_int: Fn(Int) -> Int = fn(value) { !value };\
             (invert_bool, invert_int, !'True, !0)",
            &[],
        )
        .unwrap();
        assert_eq!(
            logical_not.display(logical_not.result_type),
            "(Fn(enum {False, True}) -> enum {False, True}, Fn(Int) -> Int, enum {False, True}, Int)"
        );
    }

    #[test]
    fn intrinsic_expression_constraints_reject_invalid_or_ambiguous_operators() {
        for source in ["-\"text\"", "!1.0", "!\"text\"", "1 + 1.5"] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(
                error.message.contains("Int or Float")
                    || error.message.contains("Int or Bool")
                    || error.message.contains("cannot unify"),
                "{}",
                error.message
            );
        }

        let ambiguous =
            analyze_with_natives("let invert = fn(value) { !value }; invert", &[]).unwrap_err();
        assert!(
            ambiguous
                .message
                .contains("cannot infer monomorphic binding"),
            "{}",
            ambiguous.message
        );

        let ambiguous =
            analyze_with_natives("let negate = fn(value) { -value }; negate", &[]).unwrap_err();
        assert!(
            ambiguous
                .message
                .contains("cannot infer monomorphic binding")
        );

        let dynamic = analyze_with_natives(
            "let negate: Fn(Any) -> Any = fn(value) { -value }; negate",
            &[],
        )
        .unwrap();
        assert_eq!(dynamic.display(dynamic.result_type), "Fn(Any) -> Any");
    }

    #[test]
    fn ordered_comparisons_reject_mixed_unsupported_and_ambiguous_operands() {
        for source in ["1 < 1.0", "\"a\" < 1", "[1] < [2]"] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(
                error.message.contains("cannot unify")
                    || error.message.contains("ordered comparison"),
                "{source}: {}",
                error.message
            );
        }

        let ambiguous =
            analyze_with_natives("let before = fn(left, right) { left < right }; before", &[])
                .unwrap_err();
        assert!(
            ambiguous
                .message
                .contains("cannot infer monomorphic binding"),
            "{}",
            ambiguous.message
        );
    }

    #[test]
    fn adversarial_numeric_domains_do_not_generalize_or_merge() {
        let conflict = analyze_with_natives(
            "let add = fn(left, right) { left + right };\
             let integer = add(1, 2); add(1.0, 2.0)",
            &[],
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("cannot unify"),
            "{}",
            conflict.message
        );

        let callback = analyze_with_natives(
            "native use: for(A) Fn(Fn(Float) -> A) -> A; use(fn(value) { value + 2.0 })",
            &[("use", 1)],
        )
        .unwrap();
        assert_eq!(callback.display(callback.result_type), "Float");

        let explicit = analyze_with_natives(
            "let negate = fn(value) { -value }; negate@[String](\"x\")",
            &[],
        )
        .unwrap_err();
        assert!(
            explicit
                .message
                .contains("cannot infer monomorphic binding")
                || explicit.message.contains("monomorphic binding")
                || explicit
                    .message
                    .contains("statically known generic binding"),
            "{}",
            explicit.message
        );
    }

    #[test]
    fn branch_joins_are_canonical_pure_and_order_independent() {
        let left = analyze_with_natives("if 'True { 1 } else { \"x\" }", &[]).unwrap();
        let right = analyze_with_natives("if 'True { \"x\" } else { 1 }", &[]).unwrap();
        assert_eq!(
            left.display(left.result_type),
            right.display(right.result_type)
        );
        assert_eq!(left.display(left.result_type), "Int | String");

        let metadata = analyze_with_natives("if 'True { Int } else { String }", &[]).unwrap();
        let reversed = analyze_with_natives("if 'True { String } else { Int }", &[]).unwrap();
        assert_eq!(metadata.display(metadata.result_type), "Type");
        assert_eq!(reversed.display(reversed.result_type), "Type");

        let nested = analyze_with_natives(
            "if 'True { if 'False { 1 } else { \"x\" } } else { 1 }",
            &[],
        )
        .unwrap();
        assert_eq!(nested.display(nested.result_type), "Int | String");

        let delayed = analyze_with_natives(
            "def choose = fn(flag, value) {\
                 if flag { value } else { 1 }\
             }; let selected = choose('True, 2); choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            delayed.display(delayed.result_type),
            "Fn(enum {False, True}, Any) -> Any | Int"
        );

        let dynamic =
            analyze_with_natives("let value: Any = 1; if 'True { value } else { 1 }", &[]).unwrap();
        assert_eq!(dynamic.display(dynamic.result_type), "Any");
    }

    #[test]
    fn adversarial_branch_joins_are_pure_symmetric_and_canonical() {
        for (left, right, expected) in [
            (
                "if 'True { if 'False { 1 } else { \"x\" } } else { 1.0 }",
                "if 'True { 1.0 } else { if 'False { \"x\" } else { 1 } }",
                "Float | Int | String",
            ),
            (
                "let dynamic: Any = 1; if 'True { dynamic } else { \"x\" }",
                "let dynamic: Any = 1; if 'True { \"x\" } else { dynamic }",
                "Any",
            ),
            (
                "if 'True { Int } else { Array(String) }",
                "if 'True { Array(String) } else { Int }",
                "Type",
            ),
        ] {
            let left = analyze_with_natives(left, &[]).unwrap();
            let right = analyze_with_natives(right, &[]).unwrap();
            assert_eq!(left.display(left.result_type), expected);
            assert_eq!(right.display(right.result_type), expected);
        }

        let no_leak = analyze_with_natives(
            "let select = fn(flag, value) { if flag { value } else { 1 } };\
             (select('True, \"x\"), select('False, 2.0))",
            &[],
        )
        .unwrap();
        assert_eq!(
            no_leak.display(no_leak.result_type),
            "(Int | String, Float | Int)"
        );
    }

    #[test]
    fn structural_joins_infer_empty_collection_elements_from_sibling_branches() {
        for source in [
            "let choose = fn(flag) { if flag { {kind: 'Full, path: [1]} } else { {kind: 'Empty, path: []} } }; choose",
            "let choose = fn(flag) { if flag { {kind: 'Empty, path: []} } else { {kind: 'Full, path: [1]} } }; choose",
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(
                analysis.display(analysis.result_type),
                "Fn(enum {False, True}) -> {kind: 'Empty, path: Array<Int>} | {kind: 'Full, path: Array<Int>}"
            );
        }

        let matched = analyze_with_natives(
            "let choose = fn(value) { match value {\
                 0 => {kind: 'First, path: []},\
                 1 => {kind: 'Second, path: [1]},\
                 2 => {kind: 'Third, path: []}\
             } }; choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            matched.display(matched.result_type),
            "Fn(Any) -> {kind: 'First, path: Array<Int>} | {kind: 'Second, path: Array<Int>} | {kind: 'Third, path: Array<Int>}"
        );

        let nested = analyze_with_natives(
            "let choose = fn(flag) { if flag { 'Some(({path: []},)) } else { 'Some(({path: [1]},)) } }; choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            nested.display(nested.result_type),
            "Fn(enum {False, True}) -> 'Some(({path: Array<Int>}))"
        );

        let conflict = analyze_with_natives(
            "let choose = fn(value) { match value {\
                 0 => {kind: 'Empty, path: []},\
                 1 => {kind: 'Ints, path: [1]},\
                 2 => {kind: 'Strings, path: [\"x\"]}\
             } }; choose",
            &[],
        )
        .unwrap();
        assert_eq!(
            conflict.display(conflict.result_type),
            "Fn(Any) -> {kind: 'Empty, path: Array<Int | String>} | {kind: 'Ints, path: Array<Int>} | {kind: 'Strings, path: Array<String>}"
        );
    }

    #[test]
    fn match_joins_are_stable_across_arm_order_and_absorb_never() {
        let first = analyze_with_natives("match 0 { 0 => 1, 1 => \"x\" }", &[]).unwrap();
        let reversed = analyze_with_natives("match 0 { 1 => \"x\", 0 => 1 }", &[]).unwrap();
        assert_eq!(
            first.display(first.result_type),
            reversed.display(reversed.result_type)
        );
        assert_eq!(first.display(first.result_type), "Int | String");

        let never = analyze_with_natives(
            "native stop: Fn() -> Never;\
             if 'True { stop() } else { 1 }",
            &[("stop", 0)],
        )
        .unwrap();
        assert_eq!(never.display(never.result_type), "Int");
    }

    #[test]
    fn partial_closure_contracts_constrain_only_annotated_positions() {
        for (source, expected) in [
            (
                "let add = fn(value: Int, other) { value + other }; add",
                "Fn(Int, Int) -> Int",
            ),
            (
                "let add = fn(value) -> Int { value + 1 }; add",
                "Fn(Int) -> Int",
            ),
            (
                "let decorate = fn(ctx: Any, value) -> Int { value + 1 }; decorate",
                "Fn(Any, Int) -> Int",
            ),
            (
                "let outer = fn(value: Int) {\
                     fn(other: Int) -> Int { value + other }\
                 }; outer",
                "Fn(Int) -> Fn(Int) -> Int",
            ),
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), expected);
        }

        let compatible = analyze_with_natives(
            "let increment: Fn(Int) -> Int = fn(value: Int) -> Int { value + 1 }; increment",
            &[],
        )
        .unwrap();
        assert_eq!(compatible.display(compatible.result_type), "Fn(Int) -> Int");
    }

    #[test]
    fn partial_closure_contracts_reject_conflicts_and_invalid_metadata() {
        let conflict = analyze_with_natives(
            "let value: Fn(String) -> String = fn(value: Int) -> Int { value }; value",
            &[],
        )
        .unwrap_err();
        assert!(conflict.message.contains("cannot unify"));

        let invalid =
            analyze_with_natives("let value = fn(item: 1) { item }; value", &[]).unwrap_err();
        assert!(invalid.message.contains("closure annotation is invalid"));
    }

    #[test]
    fn explicit_type_application_instantiates_complete_generic_schemes() {
        let empty = analyze_with_natives(
            "native empty: for(A) Fn() -> Array(A); empty@[Int]()",
            &[("empty", 0)],
        )
        .unwrap();
        assert_eq!(empty.display(empty.result_type), "Array<Int>");

        let pair = analyze_with_natives(
            "native pair: for(A, B) Fn(A, B) -> B;\
             pair@[Int, String](1, \"x\")",
            &[("pair", 2)],
        )
        .unwrap();
        assert_eq!(pair.display(pair.result_type), "String");

        let computed = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A;\
             identity@[Array(Int)]([1, 2])",
            &[("identity", 1)],
        )
        .unwrap();
        assert_eq!(computed.display(computed.result_type), "Array<Int>");
    }

    #[test]
    fn partial_type_application_combines_rigid_and_inferred_arguments() {
        for (source, expected) in [
            (
                "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair@[Int, _](1, \"x\")",
                "(Int, String)",
            ),
            (
                "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair@[_, String](1, \"x\")",
                "(Int, String)",
            ),
            (
                "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair@[_, _](1, \"x\")",
                "(Int, String)",
            ),
            (
                "native empty: for(A) Fn() -> Array(A); let values: Array(Int) = empty@[_](); values",
                "Array<Int>",
            ),
            (
                "let pair = fn(left, right) { (left, right) }; pair@[Int, _](1, \"x\")",
                "(Int, String)",
            ),
        ] {
            let analysis = analyze_with_natives(source, &[("pair", 2), ("empty", 0)]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), expected);
        }

        let source = "native pair: for(A, B) Fn(A, B) -> Tuple([A, B]); pair@[Int, _](1, \"x\")";
        let analysis = analyze_with_natives(source, &[("pair", 2)]).unwrap();
        let placeholder = analysis
            .hir
            .expressions()
            .iter()
            .find(|expression| {
                expression.location.range()
                    == (source.find('_').unwrap()..source.find('_').unwrap() + 1)
            })
            .expect("placeholder expression");
        assert_eq!(
            analysis.display(analysis.expression_types[&placeholder.id]),
            "String"
        );
    }

    #[test]
    fn partial_type_application_rejects_unresolved_and_conflicting_arguments() {
        let unresolved_source = "native empty: for(A) Fn() -> Array(A); empty@[_]()";
        let unresolved = analyze_with_natives(unresolved_source, &[("empty", 0)]).unwrap_err();
        assert!(
            unresolved
                .message
                .contains("cannot infer type argument `_` for parameter \"A\""),
            "{}",
            unresolved.message
        );
        assert_eq!(
            unresolved.location.offset,
            unresolved_source.find('_').unwrap()
        );

        let never = analyze_with_natives(
            "native stop: Fn() -> Never; native identity: for(A) Fn(A) -> A; identity@[_](stop())",
            &[("stop", 0), ("identity", 1)],
        )
        .unwrap_err();
        assert!(
            never.message.contains("cannot infer type argument `_`"),
            "{}",
            never.message
        );

        let explicit_any = analyze_with_natives(
            "native empty: for(A) Fn() -> Array(A); empty@[Any]()",
            &[("empty", 0)],
        )
        .unwrap();
        assert_eq!(explicit_any.display(explicit_any.result_type), "Array<Any>");

        let conflict = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A; identity@[Int](\"x\")",
            &[("identity", 1)],
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("cannot unify"),
            "{}",
            conflict.message
        );
    }

    #[test]
    fn explicit_type_application_rejects_bad_targets_counts_and_values() {
        for (source, expected) in [
            (
                "native pair: for(A, B) Fn(A, B) -> A; pair@[Int](1, 2)",
                "expects 2 arguments, found 1",
            ),
            (
                "native identity: for(A) Fn(A) -> A; identity@[Int, String](1)",
                "expects 1 arguments, found 2",
            ),
            (
                "let identity = fn(value: Int) { value }; identity@[Int](1)",
                "statically known generic binding",
            ),
        ] {
            let error = analyze_with_natives(source, &[("pair", 2), ("identity", 1)]).unwrap_err();
            assert!(error.message.contains(expected), "{}", error.message);
        }

        let invalid = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A; identity@[1](1)",
            &[("identity", 1)],
        )
        .unwrap_err();
        assert!(
            invalid.message.contains("type argument is invalid"),
            "{}",
            invalid.message
        );
    }

    #[test]
    fn monomorphic_recursive_closures_infer_direct_mutual_and_nested_types() {
        let direct = analyze_with_natives(
            "def countdown = fn(value) {\
                 if value < 1 { 0 } else { countdown(value - 1) }\
             }; countdown",
            &[],
        )
        .unwrap();
        assert_eq!(direct.display(direct.result_type), "Fn(Int) -> Int");

        let mutual = analyze_with_natives(
            "def even = fn(value) {\
                 if value < 1 { 'True } else { odd(value - 1) }\
             };\
             def odd = fn(value) {\
                 if value < 1 { 'False } else { even(value - 1) }\
             }; (even, odd)",
            &[],
        )
        .unwrap();
        assert_eq!(
            mutual.display(mutual.result_type),
            "(Fn(Int) -> 'False | 'True, Fn(Int) -> 'False | 'True)"
        );

        let nested = analyze_with_natives(
            "{ def sum = fn(value) {\
                 if value < 1 { 0 } else { value + sum(value - 1) }\
             }; sum }",
            &[],
        )
        .unwrap();
        assert_eq!(nested.display(nested.result_type), "Fn(Int) -> Int");
    }

    #[test]
    fn acyclic_definitions_generalize_in_dependency_order() {
        for source in [
            "def identity = fn(value) { value }; (identity(1), identity(\"x\"))",
            "def identity = fn(value) { value }; def apply = fn(value) { identity(value) };\
             (apply(1), apply(\"x\"))",
            "def apply = fn(value) { identity(value) }; def identity = fn(value) { value };\
             (apply(1), apply(\"x\"))",
            "def outer = fn(value) {\
                 { def identity = fn(item) { item }; (identity(value), identity(\"x\")) }\
             }; outer(1)",
        ] {
            let analysis = analyze_with_natives(source, &[]).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "(Int, String)");
        }

        let shadowed = analyze_with_natives(
            "def identity = fn(identity) { identity }; (identity(1), identity(\"x\"))",
            &[],
        )
        .unwrap();
        assert_eq!(shadowed.display(shadowed.result_type), "(Int, String)");
    }

    #[test]
    fn acyclic_definition_aliases_instantiate_once() {
        let error = analyze_with_natives(
            "def identity = fn(value) { value }; let alias = identity;\
             (alias(1), alias(\"x\"))",
            &[],
        )
        .unwrap_err();
        assert!(error.message.contains("cannot unify"), "{}", error.message);
    }

    #[test]
    fn adversarial_alias_chains_share_one_monomorphic_instance() {
        let error = analyze_with_natives(
            "let identity = fn(value) { value }; let first = identity; let second = first;\
             let number = second(1); first(\"x\")",
            &[],
        )
        .unwrap_err();
        assert!(error.message.contains("cannot unify"), "{}", error.message);
    }

    #[test]
    fn indirect_recursive_definitions_never_publish_acyclic_schemes() {
        for source in [
            "def a = fn(value) { b(value) }; let tmp = a;\
             def b = fn(value) { tmp(value) }; let number = a(1); a(\"x\")",
            "def a = fn(value) { b(value) }; let holder = {call: a};\
             def b = fn(value) { holder.call(value) }; let number = a(1); a(\"x\")",
            "def a = fn(value) { b(value) }; let make = fn() { a };\
             def b = fn(value) { make()(value) }; let number = a(1); a(\"x\")",
        ] {
            let error = analyze_with_natives(source, &[]).unwrap_err();
            assert!(
                error
                    .message
                    .contains("indirect recursive definition requires an explicit contract"),
                "{}",
                error.message
            );
        }
    }

    #[test]
    fn recursive_inference_uses_partial_and_later_evidence_but_stays_monomorphic() {
        let partial = analyze_with_natives(
            "def countdown = fn(value: Int) -> Int {\
                 if value < 1 { 0 } else { countdown(value - 1) }\
             }; countdown",
            &[],
        )
        .unwrap();
        assert_eq!(partial.display(partial.result_type), "Fn(Int) -> Int");

        let later = analyze_with_natives(
            "def bounce = fn(value) {\
                 if 'True { value } else { bounce(value) }\
             }; let number = bounce(1); bounce",
            &[],
        )
        .unwrap();
        assert_eq!(later.display(later.result_type), "Fn(Int) -> Int");

        let conflict = analyze_with_natives(
            "def bounce = fn(value) {\
                 if 'True { value } else { bounce(value) }\
             }; let number = bounce(1); bounce(\"x\")",
            &[],
        )
        .unwrap_err();
        assert!(conflict.message.contains("cannot unify String with Int"));

        let non_closure = analyze_with_natives("def value = value; value", &[]).unwrap_err();
        assert!(non_closure.message.contains("requires a closure value"));
    }

    #[test]
    fn delayed_bindings_are_solved_monomorphically_by_later_uses() {
        let direct = analyze_with_natives(
            "def identity = fn(value) { value }; let number = identity(1); identity",
            &[],
        )
        .unwrap();
        assert_eq!(direct.display(direct.result_type), "Fn(Any) -> Any");
        assert_eq!(direct.display(direct.binding_types["number"]), "Int");

        let alias = analyze_with_natives(
            "def identity = fn(value) { value }; let alias = identity;\
             let number = alias(1); identity",
            &[],
        )
        .unwrap();
        assert_eq!(alias.display(alias.result_type), "Fn(Any) -> Any");

        let callback = analyze_with_natives(
            "native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B);\
             def identity = fn(value) { value };\
             let mapped = map([1, 2], identity); identity",
            &[("map", 2)],
        )
        .unwrap();
        assert_eq!(callback.display(callback.result_type), "Fn(Any) -> Any");

        let field = analyze_with_natives(
            "let holder = {apply: fn(value) { value }};\
             let number = holder.apply(1); holder.apply",
            &[],
        )
        .unwrap();
        assert_eq!(field.display(field.result_type), "Fn(Int) -> Int");

        let empty = analyze_with_natives(
            "native append: for(A) Fn(Array(A), A) -> Array(A);\
             let values = []; let appended = append(values, 1); values",
            &[("append", 2)],
        )
        .unwrap();
        assert_eq!(empty.display(empty.result_type), "Array<Int>");
    }

    #[test]
    fn delayed_bindings_reject_conflicts_and_underconstrained_results() {
        let independent = analyze_with_natives(
            "def identity = fn(value) { value };\
             let number = identity(1); identity(\"text\")",
            &[],
        )
        .unwrap();
        assert_eq!(independent.display(independent.result_type), "String");

        let error = analyze_with_natives("let values = []; values", &[]).unwrap_err();
        assert!(
            error.message.contains("cannot infer monomorphic binding"),
            "{}",
            error.message
        );

        let explicit = analyze_with_natives(
            "let identity: Fn(Any) -> Any = fn(value) { value }; identity",
            &[],
        )
        .unwrap();
        assert_eq!(explicit.display(explicit.result_type), "Fn(Any) -> Any");

        let recursive =
            analyze_with_natives("def recurse = fn(value) { recurse(value) }; recurse", &[]);
        assert!(recursive.is_err());
    }

    #[test]
    fn eligible_let_closures_generalize_and_instantiate_independently() {
        let analysis = analyze_with_natives(
            "let identity = fn(value) { value };\
             let wrap = fn(value) { [value] };\
             (identity(1), identity(\"text\"), wrap(2), wrap(\"value\"))",
            &[],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(Int, String, Array<Int>, Array<String>)"
        );

        let explicit =
            analyze_with_natives("let identity = fn(value) { value }; identity@[Int](1)", &[])
                .unwrap();
        assert_eq!(explicit.display(explicit.result_type), "Int");

        let exported = analyze_with_natives(
            "let identity = fn(value) { value }; {identity: identity}",
            &[],
        )
        .unwrap();
        let scheme = &exported.module_interface.exports["identity"];
        assert_eq!(scheme.parameters[0].name, "A");
        assert!(matches!(
            &scheme.body,
            TypeDescriptor::Function { parameters, result }
                if parameters == &[TypeDescriptor::Bound(TypeParameterId(0))]
                    && **result == TypeDescriptor::Bound(TypeParameterId(0))
        ));
    }

    #[test]
    fn local_generalization_respects_annotations_aliases_constraints_and_scopes() {
        let partial = analyze_with_natives(
            "let keep = fn(left: Int, right) { (left, right) };\
             (keep(1, \"x\"), keep(2, 3))",
            &[],
        )
        .unwrap();
        assert_eq!(
            partial.display(partial.result_type),
            "((Int, String), (Int, Int))"
        );

        let captures = analyze_with_natives(
            "native append: for(A) Fn(Array(A), A) -> Array(A);\
             let values = [];\
             let pair = fn(value) { (values, value) };\
             let first = pair(1); let second = pair(\"x\");\
             let appended = append(values, 2); (first, second, appended)",
            &[("append", 2)],
        )
        .unwrap();
        assert_eq!(
            captures.display(captures.result_type),
            "((Array<Int>, Int), (Array<Int>, String), Array<Int>)"
        );

        let alias = analyze_with_natives(
            "let identity = fn(value) { value }; let alias = identity;\
             (alias(1), alias(\"text\"))",
            &[],
        )
        .unwrap_err();
        assert!(alias.message.contains("cannot unify String with Int"));

        let numeric =
            analyze_with_natives("let negate = fn(value) { -value }; negate", &[]).unwrap_err();
        assert!(numeric.message.contains("cannot infer monomorphic binding"));

        let nested = analyze_with_natives(
            "let identity = fn(value) { value };\
             ({ let identity = fn(value) { [value] }; identity(1) }, identity(\"x\"))",
            &[],
        )
        .unwrap();
        assert_eq!(nested.display(nested.result_type), "(Array<Int>, String)");

        let rigid_capture = analyze_with_natives(
            "def outer: for(Outer) Fn(Outer) -> Any = fn(value) {\
                 let pair = fn(other) { (value, other) };\
                 (pair(1), pair(\"x\"))\
             }; outer(0)",
            &[],
        )
        .unwrap();
        let pair = rigid_capture
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "pair")
            .unwrap();
        assert_eq!(
            rigid_capture.definition_schemes[&pair.id].display_name(),
            "for(A) Fn(A) -> (T0, A)"
        );
    }

    #[test]
    fn generic_contract_parameters_are_available_in_implementation_annotations() {
        let analysis = analyze_with_natives(
            "type Pair(Left, Right) = struct {left: Left, right: Right};\
             type Box(Content) = struct {value: Content};\
             def collect: for(N, M) Fn(Array(Box(Pair(N, M)))) -> Array(Box(Pair(N, M))) = fn(items) {\
                 let result: Array(Box(Pair(N, M))) = items;\
                 let retain = fn(values: Array(Box(Pair(N, M)))) -> Array(Box(Pair(N, M))) { values };\
                 retain(result)\
             };\
             collect",
            &[],
        )
        .unwrap();
        let collect = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "collect")
            .unwrap();
        assert_eq!(
            analysis.definition_schemes[&collect.id].display_name(),
            "for(N, M) Fn(Array<Box>) -> Array<Box>"
        );

        let leaked = analyze_with_natives(
            "def identity: for(N) Fn(N) -> N = fn(value) {\
                 let result: N = value; result\
             };\
             let unrelated: N = 1; unrelated",
            &[],
        )
        .unwrap_err();
        assert!(
            leaked.message.contains("unknown binding \"N\""),
            "{}",
            leaked.message
        );
    }

    #[test]
    fn nested_closures_share_only_body_constraints() {
        let analysis = analyze_with_natives(
            "let nested = fn(left) { fn(right) { left + right + 1 } }; nested",
            &[],
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "Fn(Int) -> Fn(Int) -> Int"
        );
    }

    #[test]
    fn ordinary_expressions_use_bidirectional_checking_without_schemes() {
        let analysis = analyze_with_natives(
            "let values: Array(Int) = if 'True { [] } else { [1] };\
             let selected: Int = match (1, \"x\") { (number, _) => number };\
             (values, selected)",
            &[],
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "(Array<Int>, Int)");

        let error = analyze_with_natives(
            "def broken: Fn(Int) -> Int = fn(value) { value + \"x\" }; broken",
            &[],
        )
        .unwrap_err();
        assert!(error.message.contains("cannot unify String with Int"));

        let nested =
            analyze_with_natives("let outer = { let value: Int = \"x\"; value }; outer", &[])
                .unwrap_err();
        assert!(nested.message.contains("cannot unify String with Int"));
    }

    #[test]
    fn generic_native_parameters_must_be_unique() {
        let error = analyze_with_natives(
            "native identity: for(A, A) Fn(A) -> A; identity(1)",
            &[("identity", 1)],
        )
        .unwrap_err();
        assert!(error.message.contains("duplicate type parameter"));

        let leaked =
            analyze_with_natives("native identity: for(A) Fn(A) -> A; A", &[("identity", 1)])
                .unwrap_err();
        assert!(leaked.message.contains("unknown binding \"A\""));
    }

    #[test]
    fn generic_native_schemes_are_data_and_occurs_checks_reject_infinite_types() {
        let analysis = analyze_with_natives(
            "native identity: for(A) Fn(A) -> A; {identity: identity}",
            &[("identity", 1)],
        )
        .unwrap();
        let scheme = &analysis.module_interface.exports["identity"];
        assert_eq!(scheme.parameters[0].name, "A");
        assert!(matches!(
            &scheme.body,
            TypeDescriptor::Function { parameters, result }
                if parameters == &[TypeDescriptor::Bound(TypeParameterId(0))]
                    && **result == TypeDescriptor::Bound(TypeParameterId(0))
        ));

        let schemes = HashMap::new();
        let interfaces = BTreeMap::new();
        let annotations = HashMap::new();
        let named_types = BTreeMap::new();
        let hir = HirProgram::default();
        let mut inference = GenericInference::new(
            &schemes,
            &hir,
            &interfaces,
            &named_types,
            &annotations,
            true,
            None,
        );
        let variable = TypeDescriptor::Inference(InferenceVariableId(0));
        assert!(
            inference
                .unify(
                    &variable,
                    &TypeDescriptor::Array(Box::new(variable.clone()))
                )
                .unwrap_err()
                .contains("infinite type")
        );
    }

    #[test]
    fn published_schemes_reject_solver_and_unbound_parameter_identities() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("scheme.telora", "");
        let location = crate::Location::from_usize(source, 0..0).unwrap();
        let valid = TypeScheme {
            parameters: vec![TypeParameter {
                id: TypeParameterId(0),
                name: "A".into(),
                location,
            }],
            body: TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Bound(TypeParameterId(0))],
                result: Box::new(TypeDescriptor::Bound(TypeParameterId(0))),
            },
        };
        assert!(validate_publishable_scheme(&valid).is_ok());

        let unresolved = TypeScheme {
            parameters: Vec::new(),
            body: TypeDescriptor::Inference(InferenceVariableId(0)),
        };
        assert!(
            validate_publishable_scheme(&unresolved)
                .unwrap_err()
                .contains("unresolved")
        );

        let unbound = TypeScheme {
            parameters: Vec::new(),
            body: TypeDescriptor::Bound(TypeParameterId(7)),
        };
        assert!(
            validate_publishable_scheme(&unbound)
                .unwrap_err()
                .contains("unbound parameter T7")
        );
    }

    #[test]
    #[should_panic(expected = "solver descriptors must be explicitly erased before interning")]
    fn strict_type_graph_interning_rejects_solver_descriptors() {
        TypeGraph::default().intern_descriptor(&TypeDescriptor::Inference(InferenceVariableId(0)));
    }

    #[test]
    fn explicit_runtime_erasure_is_the_only_solver_to_any_path() {
        let mut types = TypeGraph::default();
        let erased = types.intern_erased_descriptor(&TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Bound(TypeParameterId(0))],
            result: Box::new(TypeDescriptor::Inference(InferenceVariableId(0))),
        });
        assert_eq!(types.display(erased), "Fn(Any) -> Any");
    }

    #[test]
    fn metadata_round_trips() {
        let descriptor = TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Struct(BTreeMap::from([
                ("age".into(), TypeDescriptor::Int),
                ("name".into(), TypeDescriptor::String),
            ]))],
            result: Box::new(TypeDescriptor::Enum(BTreeMap::from([
                ("None".into(), None),
                ("Some".into(), Some(Box::new(TypeDescriptor::String))),
            ]))),
        };
        let value = descriptor.to_value(&mut Vm::new());
        assert!(matches!(
            &value,
            Value::Dict(fields)
                if matches!(fields.get("kind"), Some(Value::Atom(kind)) if kind.name() == "Func")
        ));
        assert_eq!(TypeDescriptor::from_value(&value).unwrap(), descriptor);

        let bound = TypeDescriptor::Array(Box::new(TypeDescriptor::Bound(TypeParameterId(7))));
        let value = bound.to_value(&mut Vm::new());
        assert_eq!(TypeDescriptor::from_value(&value).unwrap(), bound);

        let metatype = TypeDescriptor::Type;
        let value = metatype.to_value(&mut Vm::new());
        assert_eq!(TypeDescriptor::from_value(&value).unwrap(), metatype);

        let never = TypeDescriptor::Never;
        let value = never.to_value(&mut Vm::new());
        assert_eq!(TypeDescriptor::from_value(&value).unwrap(), never);

        let witness = TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Array(Box::new(
            TypeDescriptor::Int,
        ))));
        let value = witness.to_value(&mut Vm::new());
        assert_eq!(TypeDescriptor::from_value(&value).unwrap(), witness);
    }

    #[test]
    fn metadata_values_and_typed_constructors_have_the_type_metatype() {
        let analysis = analyze_source(
            "metatype.telora",
            "def Maybe: for(A) Fn(TypeOf(A)) -> TypeOf(Option(A)) = fn(Item) { Option(Item) };\
             type MaybeInt = Maybe(Int);\
             (Type, Int, Array(Int), Maybe)",
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.result_type),
            "(TypeOf(Type), TypeOf(Int), TypeOf(Array<Int>), Fn(TypeOf(Any)) -> TypeOf(enum {None, Some(Any)}))"
        );
        let maybe_int = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "MaybeInt")
            .expect("MaybeInt definition");
        assert_eq!(
            analysis.display(analysis.definition_types[&maybe_int.id]),
            "TypeOf(enum {None, Some(Int)})"
        );
        assert!(matches!(
            analysis.types.node(analysis.declared_types["MaybeInt"]),
            TypeNode::Enum(_)
        ));

        let bad_argument = analyze_source(
            "bad-argument.telora",
            "def Broken: Fn(Type) -> Type = fn(Item) { Array(1) }; Broken",
        )
        .unwrap_err();
        assert!(bad_argument.message.contains("cannot unify Int with Type"));

        let bad_result = analyze_source(
            "bad-result.telora",
            "def Broken: Fn(Type) -> Type = fn(Item) { 1 }; Broken",
        )
        .unwrap_err();
        assert!(bad_result.message.contains("cannot unify Int with Type"));
    }

    #[test]
    fn fn_notation_and_func_constructor_share_canonical_metadata() {
        let definitions = analyze_source(
            "definitions.telora",
            "def make: Fn(Int) -> Tuple([Int, String]) = fn(value) { (value, \"ok\") };\
             decl copy: Fn(Int) -> Tuple([Int, String]);\
             def copy = make;\
             (make(1), copy(2))",
        )
        .unwrap();
        assert_eq!(
            definitions.display(definitions.result_type),
            "((Int, String), (Int, String))"
        );

        let native = analyze_with_natives(
            "native convert: Fn(Int) -> Array(Tuple([String, Int])); convert(1)",
            &[("convert", 1)],
        )
        .unwrap();
        assert_eq!(native.display(native.result_type), "Array<(String, Int)>");

        let explicit = analyze_source(
            "explicit.telora",
            "type ViaSyntax = Func([Int], String);\
             def value: ViaSyntax = fn(number) { if number == 0 { \"zero\" } else { \"other\" } };\
             value",
        )
        .unwrap();
        assert_eq!(
            explicit.display(explicit.declared_types["ViaSyntax"]),
            "Fn(Int) -> String"
        );
    }

    #[test]
    fn tuple_contracts_do_not_rewrite_constructor_arity() {
        let error = analyze_with_natives(
            "native invalid: Fn(Int) -> Tuple(Int, String); invalid(1)",
            &[("invalid", 1)],
        )
        .unwrap_err();
        assert!(
            error.message.contains("expected 1 arguments, got 2"),
            "{error}"
        );
    }

    #[test]
    fn function_remains_available_as_a_domain_type_name() {
        let analysis = analyze_source(
            "domain-name.telora",
            "type Function = Int; let value: Function = 1; value",
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "Int");
    }

    #[test]
    fn parameterized_type_family_publishes_a_precise_constructor_scheme() {
        let analysis = analyze_source(
            "family.telora",
            "type Box(A) = struct {value: A};\
             type IntBox = Box(Int);\
             def wrap: for(A) Fn(A) -> Box(A) = fn(value) { {value} };\
             wrap(1)",
        )
        .unwrap();
        let box_definition = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "Box")
            .expect("Box definition");
        assert_eq!(
            analysis.definition_schemes[&box_definition.id].display_name(),
            "for(A) Fn(TypeOf(A)) -> TypeOf(Box)"
        );
        assert_eq!(analysis.display(analysis.declared_types["IntBox"]), "Box");
        assert_eq!(analysis.display(analysis.result_type), "Box");
    }

    #[test]
    fn declared_family_applications_use_head_and_argument_identity() {
        let analysis = analyze_source(
            "family-identities.telora",
            "type Box(A) = struct {value: A};\
             type Other(A) = struct {value: A};\
             type Phantom(A) = struct {value: Int};\
             type Maybe(A) = enum {'None, 'Some(A)};\
             type IntBox = Box(Int);\
             type IntBoxAlias = Box(Int);\
             type Text = Box(String);\
             type EqualShape = Other(Int);\
             type PhantomInt = Phantom(Int);\
             type PhantomText = Phantom(String);\
             type Nested = Box(Maybe(Int));\
             type Optional = Maybe(Int);\
             0",
        )
        .unwrap();
        let declared_id = |name: &str| {
            let TypeNode::Declared { id, .. } = analysis.types.node(analysis.declared_types[name])
            else {
                panic!("{name} must be a declared family application")
            };
            id
        };

        assert_eq!(declared_id("IntBox"), declared_id("IntBoxAlias"));
        assert_ne!(declared_id("IntBox"), declared_id("Text"));
        assert_ne!(declared_id("IntBox"), declared_id("EqualShape"));
        assert_ne!(declared_id("PhantomInt"), declared_id("PhantomText"));
        assert_eq!(declared_id("Nested").arguments().len(), 1);
        assert_eq!(declared_id("Optional").arguments().len(), 1);
        assert_ne!(
            declared_id("Nested").identity_key(),
            declared_id("Optional").identity_key()
        );
    }

    #[test]
    fn generic_calls_infer_parameters_through_struct_family_arguments() {
        let analysis = analyze_source(
            "family-argument.telora",
            "type Box(Content) = struct {value: Content};\
             def unbox: for(Content) Fn(Box(Content)) -> Content = fn(boxed) { boxed.value };\
             let boxed: Box(Int) = {value: 7};\
             let value: Int = unbox(boxed);\
             let inferred = unbox(boxed);\
             (value, inferred)",
        )
        .unwrap();
        let unbox = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "unbox")
            .expect("unbox definition");
        assert_eq!(
            analysis.definition_schemes[&unbox.id].display_name(),
            "for(Content) Fn(Box) -> Content"
        );
        assert_eq!(analysis.display(analysis.result_type), "(Int, Int)");
    }

    #[test]
    fn generic_call_context_widens_singleton_fields_in_anonymous_records() {
        let prelude = "type Node = enum {'A, 'B};\
             type Requirement = struct {target: Node};\
             def target_of: Fn(Requirement) -> Node = fn(req) { req.target };\
             def use: for(Req) Fn(Array(Req), Fn(Req) -> Node) -> Node =\
                 fn(requirements, selector) { selector(requirements[0]) };";
        for records in ["[{target: 'B}]", "[{target: 'A}, {target: 'B}]"] {
            let analysis = analyze_source(
                "generic-record-widening.telora",
                &format!("{prelude} use({records}, target_of)"),
            )
            .unwrap();
            assert_eq!(analysis.display(analysis.result_type), "Node");
        }

        let conflict = analyze_source(
            "generic-record-conflict.telora",
            &format!("{prelude} use([{{target: 'Foreign}}], target_of)"),
        )
        .unwrap_err();
        assert!(
            conflict.message.contains("Foreign") && conflict.message.contains("enum {A, B}"),
            "{}",
            conflict.message
        );

        let conflicting_enum = analyze_source(
            "generic-record-enum-conflict.telora",
            &format!(
                "{prelude} type Foreign = enum {{'Foreign}};\
                 let foreign: Foreign = 'Foreign;\
                 use([{{target: foreign}}], target_of)"
            ),
        )
        .unwrap_err();
        assert!(
            conflicting_enum.message.contains("Foreign")
                && conflicting_enum.message.contains("Node"),
            "{}",
            conflicting_enum.message
        );
    }

    #[test]
    fn generic_struct_families_construct_nested_array_tuple_fields() {
        let analysis = analyze_source(
            "nested-family-field.telora",
            "type Box(A) = struct {value: Array(Tuple([A, Int]))};\
             def make: for(A) Fn(Array(Tuple([A, Int]))) -> Box(A) =\
                 fn(value) { {value} };\
             make([(1, 2)]).value",
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "Array<(Int, Int)>");

        for source in [
            "type Box(A) = struct {value: Array(Tuple([Int, A]))};\
             def make: for(A) Fn(Array(Tuple([Int, A]))) -> Box(A) =\
                 fn(value) { {value} };\
             make([(1, \"two\")]).value",
            "type Box(A) = struct {value: Array(Tuple([Int, A, String]))};\
             def make: for(A) Fn(Array(Tuple([Int, A, String]))) -> Box(A) =\
                 fn(value) { {value} };\
             make([(1, 2.0, \"three\")]).value",
        ] {
            analyze_source("nested-family-position.telora", source).unwrap();
        }

        let incompatible = analyze_source(
            "incompatible-nested-family-field.telora",
            "type Box(A) = struct {value: Array(Tuple([A, Int]))};\
             def make: for(A) Fn(Array(Tuple([A, String]))) -> Box(A) =\
                 fn(value) { {value} };\
             make([(1, \"wrong\")])",
        )
        .unwrap_err();
        assert!(
            incompatible.message.contains("String") && incompatible.message.contains("Int"),
            "{}",
            incompatible.message
        );

        let shadowed = analyze_source(
            "shadowed-tuple.telora",
            "let Tuple = fn(value) { value }; Tuple([1, 2])",
        )
        .unwrap();
        assert_eq!(shadowed.display(shadowed.result_type), "Array<Int>");
    }

    #[test]
    fn explicit_array_context_checks_anonymous_concrete_family_catalogs() {
        let definitions = "type Id = enum {'First, 'Second};\
             type Mode = enum {'Direct, 'Derived};\
             type Capability(IdType, Input, Output) = struct {\
                 id: IdType,\
                 mode: Mode,\
                 lower: Fn(Input) -> Option(Output),\
                 dependencies: Array(IdType),\
             };\
             type Concrete = Capability(Id, Int, String);";
        let first = "{\
            id: 'First,\
            mode: 'Direct,\
            lower: fn(value) { 'Some(`value=\\{value}`) },\
            dependencies: [],\
        }";
        let second = "{\
            id: 'Second,\
            mode: 'Derived,\
            lower: fn(value) { if value == 0 { 'None } else { 'Some(`value=\\{value}`) } },\
            dependencies: ['First],\
        }";

        for (name, elements) in [
            ("forward", format!("{first}, {second}")),
            ("reverse", format!("{second}, {first}")),
        ] {
            let source =
                format!("{definitions} let catalog: Array(Concrete) = [{elements}]; catalog");
            let analysis = analyze_source(&format!("catalog-{name}.telora"), &source).unwrap();
            assert_eq!(analysis.display(analysis.result_type), "Array<Capability>");
        }

        let incompatible = analyze_source(
            "catalog-incompatible.telora",
            &format!(
                "{definitions} let catalog: Array(Concrete) = [{first}, {{\
                    id: 'Second,\
                    mode: 'Derived,\
                    lower: fn(value) {{ 'Some(value) }},\
                    dependencies: ['First],\
                }}]; catalog"
            ),
        )
        .unwrap_err();
        assert!(
            incompatible.message.contains("String") && incompatible.message.contains("Int"),
            "{}",
            incompatible.message
        );
    }

    #[test]
    fn parameterized_type_families_compose_symbolic_templates() {
        let analysis = analyze_source(
            "families.telora",
            "type Box(A) = struct {value: A};\
             type Envelope(Payload, Error) = struct {\
                 payload: Option(Box(Payload)),\
                 error: Option(Error),\
             };\
             type Response = Envelope(String, Int);\
             Response",
        )
        .unwrap();
        let response = analysis.declared_types["Response"];
        assert_eq!(analysis.display(response), "Envelope");
        let TypeNode::Declared { body, .. } = analysis.types.node(response) else {
            panic!("Response must retain its Envelope owner")
        };
        let body = analysis.types.display(*body);
        assert!(body.contains("Box"), "{body}");
        assert!(body.contains("Int"), "{body}");
        assert!(!body.contains("Any"), "{body}");
    }

    #[test]
    fn parameterized_type_families_evaluate_in_dependency_order() {
        let analysis = analyze_source(
            "forward-family.telora",
            "type Outer(A) = Inner(A);\
             type Inner(A) = Array(A);\
             type Output = Outer(String);\
             Output",
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.declared_types["Output"]),
            "Array<String>"
        );
    }

    #[test]
    fn parameterized_type_families_capture_acyclic_local_concrete_types() {
        for (name, source) in [
            (
                "earlier-concrete.telora",
                "type Id = Int;\
                 type Pair(A) = Tuple([Id, A]);\
                 type Output = Pair(String);\
                 Output",
            ),
            (
                "later-concrete.telora",
                "type Pair(A) = Tuple([Id, A]);\
                 type Id = Int;\
                 type Output = Pair(String);\
                 Output",
            ),
            (
                "concrete-family-chain.telora",
                "type Outer(A) = Tuple([Local, A]);\
                 type Local = Inner(String);\
                 type Inner(A) = Tuple([A, Int]);\
                 type Output = Outer(Float);\
                 Output",
            ),
        ] {
            let analysis = analyze_source(name, source).unwrap();
            let expected = if name == "concrete-family-chain.telora" {
                "((String, Int), Float)"
            } else {
                "(Int, String)"
            };
            assert_eq!(
                analysis.display(analysis.declared_types["Output"]),
                expected,
                "{name}"
            );
        }
    }

    #[test]
    fn concrete_types_schedule_family_applications_with_later_concrete_arguments() {
        let analysis = analyze_source(
            "decorated-concrete-family-chain.telora",
            "type Requirement(E) = struct {target: E, reason: String};\
             type Output = struct {requirements: Array(Requirement(Entity))};\
             type Entity = enum {'Order};\
             Output",
        )
        .unwrap();
        assert_eq!(
            analysis.display(analysis.declared_types["Output"]),
            "Output"
        );
    }

    #[test]
    fn concrete_family_dependency_scheduling_is_source_order_independent_and_transitive() {
        let mut outputs = Vec::new();
        for (name, source) in [
            (
                "earlier-concrete-argument.telora",
                "type Entity = enum {'Order};\
                 type Requirement(E) = struct {target: E, reason: String};\
                 type Output = struct {requirements: Array(Requirement(Entity))}; Output",
            ),
            (
                "multilevel-concrete-family-chain.telora",
                "type Requirement(E) = struct {target: E, reason: String};\
                 type Requirements(E) = Array(Requirement(E));\
                 type Output = struct {requirements: Requirements(Entity)};\
                 type Entity = enum {'Order}; Output",
            ),
        ] {
            let analysis = analyze_source(name, source).unwrap();
            outputs.push((name, analysis.display(analysis.declared_types["Output"])));
        }
        assert!(outputs.iter().all(|(_, output)| output == &outputs[0].1));
    }

    #[test]
    fn type_aliases_preserve_recursive_concrete_family_arguments() {
        let analysis = analyze_source(
            "recursive-family-alias.telora",
            "type Box(A) = struct {value: A};\
             type Branch = struct {children: Array(Tree)};\
             type Tree = enum {'Leaf(Int), 'Branch(Branch)};\
             type TreeBox = Box(Tree);\
             def identity: Fn(TreeBox) -> TreeBox = fn(value) { value };\
             identity({value: 'Leaf(1)})",
        )
        .unwrap();
        let alias = analysis.declared_types["TreeBox"];
        assert_eq!(analysis.display(alias), "Box");
        let TypeNode::Declared { body, .. } = analysis.types.node(alias) else {
            panic!("a family application must retain its declared owner")
        };
        let body = analysis.types.display(*body);
        assert!(body.contains("Tree"), "{body}");
        assert!(!body.contains("Any"), "{body}");
        assert!(!analysis.display(analysis.result_type).contains("Any"));
    }

    #[test]
    fn parameterized_type_family_diagnostics_preserve_bounded_failures() {
        let duplicate = analyze_source(
            "duplicate-family.telora",
            "type Pair(A, A) = Tuple([A, A]); 0",
        )
        .unwrap_err();
        assert!(duplicate.message.contains("duplicate type parameter \"A\""));

        let arity = analyze_source(
            "arity-family.telora",
            "type Box(A) = Array(A); type Broken = Box(Int, String); 0",
        )
        .unwrap_err();
        assert!(
            arity.message.contains("expected 1 arguments, got 2"),
            "{}",
            arity.message
        );

        let invalid = analyze_source("invalid-family.telora", "type Broken(A) = 1; 0").unwrap_err();
        assert!(invalid.message.contains("produced invalid metadata"));

        let direct =
            analyze_source("recursive-family.telora", "type Loop(A) = Loop(A); 0").unwrap_err();
        assert!(direct.message.contains("recursive type family component"));

        let mutual = analyze_source(
            "mutual-family.telora",
            "type Left(A) = Right(A); type Right(A) = Left(A); 0",
        )
        .unwrap_err();
        assert!(mutual.message.contains("recursive type family component"));

        let mixed = analyze_source(
            "mixed-recursive-family.telora",
            "type Family(A) = Tuple([Concrete, A]);\
             type Concrete = Family(Int);\
             0",
        )
        .unwrap_err();
        assert!(
            mixed.message.contains("recursive type family component")
                && mixed.message.contains("Family")
                && mixed.message.contains("Concrete"),
            "{}",
            mixed.message
        );
        let diagnostic = mixed.diagnostic.expect("mixed cycle diagnostic");
        assert_eq!(diagnostic.labels.len(), 2);
    }

    #[test]
    fn type_validation_uses_the_authoritative_metadata_decoder() {
        let valid =
            crate::compile_source("valid-type.telora", "validate(Type, Array(Int))").unwrap();
        assert!(
            Vm::new()
                .execute(&valid, 100_000)
                .unwrap()
                .to_string()
                .starts_with("'Ok(")
        );

        let invalid = crate::compile_source(
            "invalid-type.telora",
            "validate(Type, {kind: 'Array, item: 1})",
        )
        .unwrap();
        let output = Vm::new().execute(&invalid, 100_000).unwrap().to_string();
        assert!(output.starts_with("'Err("), "{output}");
        assert!(output.contains("value.item must be a Dict"), "{output}");
    }

    #[test]
    fn ordinary_closure_computes_type_metadata() {
        let analysis = analyze_source(
            "test",
            "def Optional = fn(item) { union('None, [Atom('None), Tagged('Some, item)]) };\
             type MaybeInt = Optional(Int);\
             let value: MaybeInt = 'Some(42);\
             value",
        )
        .unwrap();
        let maybe = analysis.declared_types.get("MaybeInt").unwrap();
        assert!(
            matches!(analysis.types.node(*maybe), TypeNode::Union(variants) if variants.len() == 2)
        );
    }

    #[test]
    fn reports_structural_annotation_mismatch() {
        let error = analyze_source(
            "test",
            "type User = struct {name: String, age: Int};\
             let user: User = {name: \"Ada\", age: \"old\"};\
             user",
        )
        .unwrap_err();
        assert!(
            error.message.contains("String") && error.message.contains("Int"),
            "{}",
            error.message
        );
    }

    #[test]
    fn checks_interpolation_inside_nested_binding_annotations() {
        let error = analyze_source("test", r#"let outer = { let x: `\{[1]}` = "x"; x }; outer"#)
            .unwrap_err();
        assert!(error.message.contains("does not support Array<Int>"));
    }

    #[test]
    fn records_a_type_fact_for_every_resolved_hir_expression() {
        let analysis = analyze_source(
            "facts.telora",
            "let values = [1, 2]; let first = fn(x) { let y = x; y }; first(values)",
        )
        .unwrap();
        assert_eq!(
            analysis.expression_types.len(),
            analysis.hir.expressions().len()
        );
        assert!(
            analysis
                .expression_types
                .values()
                .any(|ty| matches!(analysis.types.node(*ty), TypeNode::Int))
        );
        assert!(
            analysis
                .expression_types
                .values()
                .any(|ty| matches!(analysis.types.node(*ty), TypeNode::Array(_)))
        );
        assert!(
            analysis
                .expression_types
                .values()
                .any(|ty| matches!(analysis.types.node(*ty), TypeNode::Function { .. }))
        );
    }

    #[test]
    fn struct_patterns_bind_field_types_and_reject_unknown_fields() {
        let analysis = analyze_source(
            "pattern.telora",
            "type User = struct {name: String, age: Int};\
             let user: User = {name: \"Ada\", age: 36};\
             match user { {age} => age + 1 }",
        )
        .unwrap();
        assert_eq!(analysis.display(analysis.result_type), "Int");
        let age = analysis
            .hir
            .definitions()
            .iter()
            .find(|definition| {
                definition.kind == HirDefinitionKind::Pattern && definition.name == "age"
            })
            .unwrap();
        assert_eq!(analysis.display(analysis.definition_types[&age.id]), "Int");

        let error = analyze_source(
            "pattern.telora",
            "type User = struct {name: String};\
             let user: User = {name: \"Ada\"};\
             match user { {age} => age }",
        )
        .unwrap_err();
        assert!(error.to_string().contains("Struct has no field \"age\""));

        let wrong_shape =
            analyze_source("pattern.telora", "match 1 { {} => 0, _ => 1 }").unwrap_err();
        assert!(
            wrong_shape
                .to_string()
                .contains("Struct pattern cannot match Int")
        );

        let duplicate = analyze_source(
            "pattern.telora",
            "let user = {name: \"Ada\"}; match user { {name, name} => name }",
        )
        .unwrap_err();
        assert!(
            duplicate
                .to_string()
                .contains("duplicate Struct pattern field \"name\""),
            "{duplicate}"
        );
    }

    #[test]
    fn closed_enum_matches_require_conservative_whole_variant_coverage() {
        let complete = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1);\
             match option { 'None => 0, 'Some(value) => value }",
        )
        .unwrap();
        assert_eq!(complete.display(complete.result_type), "Int");

        let missing = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1); match option { 'Some(value) => value }",
        )
        .unwrap_err();
        assert!(missing.to_string().contains("missing 'None"), "{missing}");

        let refutable = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1);\
             match option { 'None => 0, 'Some(1) => 1 }",
        )
        .unwrap_err();
        assert!(
            refutable.to_string().contains("missing 'Some(_)"),
            "{refutable}"
        );

        let catch_all = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1); match option { _ => 0 }",
        );
        assert!(catch_all.is_ok());

        let dynamic = analyze_source(
            "match.telora",
            "let inspect: Fn(Any) -> Int = fn(value) { match value { 'None => 0 } }; inspect",
        );
        assert!(dynamic.is_ok());
    }

    #[test]
    fn redundant_match_arms_require_certain_prior_coverage() {
        let incompatible =
            analyze_source("match.telora", "match (1, \"x\") { (left, 2) => left }").unwrap_err();
        assert!(
            incompatible
                .to_string()
                .contains("pattern cannot match (Int, String)"),
            "{incompatible}"
        );

        let after_catch_all = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'None; match option { _ => 0, 'None => 1 }",
        )
        .unwrap_err();
        assert!(
            after_catch_all
                .to_string()
                .contains("prior arms cover every value"),
            "{after_catch_all}"
        );

        let repeated = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'None;\
             match option { 'None => 0, 'None => 1, 'Some(_) => 2 }",
        )
        .unwrap_err();
        assert!(repeated.to_string().contains("cover 'None"), "{repeated}");

        let covered_payload = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1);\
             match option { 'Some(_) => 0, 'Some(1) => 1, 'None => 2 }",
        )
        .unwrap_err();
        assert!(
            covered_payload.to_string().contains("cover 'Some"),
            "{covered_payload}"
        );

        let distinct_partial = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'Some(1);\
             match option { 'None => 0, 'Some(1) => 1, 'Some(2) => 2, 'Some(_) => 3 }",
        );
        assert!(distinct_partial.is_ok(), "{distinct_partial:?}");

        let complete_then_catch_all = analyze_source(
            "match.telora",
            "let option: Option(Int) = 'None;\
             match option { 'None => 0, 'Some(_) => 1, _ => 2 }",
        )
        .unwrap_err();
        assert!(
            complete_then_catch_all
                .to_string()
                .contains("prior arms cover every value"),
            "{complete_then_catch_all}"
        );

        let struct_then_arm = analyze_source(
            "match.telora",
            "let user = {name: \"Ada\"}; match user { {name} => name, _ => \"none\" }",
        )
        .unwrap_err();
        assert!(
            struct_then_arm
                .to_string()
                .contains("prior arms cover every value"),
            "{struct_then_arm}"
        );
    }

    #[test]
    fn destructuring_let_requires_irrefutable_known_shapes() {
        let valid = analyze_source(
            "let.telora",
            "{ let (count, {name}) = (1, {name: \"Ada\"}); (count, name) }",
        )
        .unwrap();
        assert_eq!(valid.display(valid.result_type), "(Int, String)");
        let name = valid
            .hir
            .definitions()
            .iter()
            .find(|definition| {
                definition.kind == HirDefinitionKind::Pattern && definition.name == "name"
            })
            .unwrap();
        assert_eq!(valid.display(valid.definition_types[&name.id]), "String");

        let wrong_arity =
            analyze_source("let.telora", "{ let (left, right) = (1,); left }").unwrap_err();
        assert!(
            wrong_arity
                .to_string()
                .contains("refutable let pattern for (Int)"),
            "{wrong_arity}"
        );

        let dynamic = analyze_source(
            "let.telora",
            "let pair: Any = (1, 2); { let (left, right) = pair; left }",
        )
        .unwrap_err();
        assert!(
            dynamic
                .to_string()
                .contains("refutable let pattern for Any"),
            "{dynamic}"
        );

        let nested = analyze_source(
            "let.telora",
            "let option: Option(Int) = 'Some(1);\
             { let (first, 'Some(value)) = (0, option); value }",
        )
        .unwrap_err();
        assert!(
            nested
                .to_string()
                .contains("refutable let pattern for (Int, enum"),
            "{nested}"
        );
    }

    #[test]
    fn partial_type_evaluation_continues_independent_and_transitive_work() {
        let partial = analyze_partial_types(
            "partial.telora",
            "type A = broken(Int);\
             type B = String;\
             type C = Array(B);\
             type D = Array(A);\
             0",
            Quota::with_fuel(100),
        );
        let definition = |name: &str| {
            partial
                .hir
                .definitions()
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .id
        };
        let a = definition("A");
        let b = definition("B");
        let c = definition("C");
        let d = definition("D");
        assert!(matches!(
            partial.definition_facts[&a].state,
            FactState::Incomputable(IncomputableReason::UnsupportedOperation)
        ));
        assert_eq!(partial.definition_facts[&b].state, FactState::Known);
        assert_eq!(partial.definition_facts[&c].state, FactState::Known);
        assert_eq!(
            partial
                .types
                .display(partial.definition_facts[&c].value.unwrap()),
            "Array<String>"
        );
        assert_eq!(
            partial.definition_facts[&d].state,
            FactState::Unknown(UnknownReason::BlockedBy(FactIdentity::HirDefinition(a)))
        );
        assert!(partial.definition_facts[&d].diagnostics.is_empty());
        assert_eq!(partial.diagnostics.len(), 1);
        let c_node = partial
            .dependencies
            .nodes
            .iter()
            .find(|node| node.definition == c)
            .unwrap();
        assert_eq!(c_node.dependencies, vec![b]);
    }

    #[test]
    fn partial_type_evaluation_resolves_local_concrete_family_dependencies() {
        let partial = analyze_partial_types(
            "partial-family.telora",
            "type Result(A) = Tuple([Outcome, A]);\
             type Outcome = Int;\
             type Output = Result(String);\
             type Independent = Float;\
             0",
            Quota::with_fuel(100),
        );
        let definition = |name: &str| {
            partial
                .hir
                .definitions()
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .id
        };
        for name in ["Result", "Outcome", "Output", "Independent"] {
            assert_eq!(
                partial.definition_facts[&definition(name)].state,
                FactState::Known,
                "{name}"
            );
        }
        assert_eq!(
            partial.types.display(
                partial.definition_facts[&definition("Output")]
                    .value
                    .unwrap()
            ),
            "(Int, String)"
        );
        assert_eq!(partial.diagnostics, Vec::<Diagnostic>::new());
    }

    #[test]
    fn partial_type_evaluation_shares_one_fuel_account() {
        let partial = analyze_partial_types(
            "fuel.telora",
            "type A = Array(Int); type B = Array(Int); 0",
            Quota::with_fuel(1),
        );
        let facts = partial
            .hir
            .definitions()
            .iter()
            .filter(|definition| definition.kind == HirDefinitionKind::Type)
            .map(|definition| &partial.definition_facts[&definition.id])
            .collect::<Vec<_>>();
        assert_eq!(facts[0].state, FactState::Known);
        assert_eq!(
            facts[1].state,
            FactState::Incomputable(IncomputableReason::QuotaExceeded)
        );
    }

    #[test]
    fn partial_type_evaluation_seals_decorated_recursive_components() {
        let partial = analyze_partial_types(
            "recursive.telora",
            "type Node = struct {children: Array(Node)}; 0",
            Quota::with_fuel(100),
        );
        let node = partial
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "Node")
            .unwrap();
        assert_eq!(partial.definition_facts[&node.id].state, FactState::Known);
        assert_eq!(partial.dependencies.nodes[0].dependencies, vec![node.id]);
        assert!(partial.diagnostics.is_empty());
        assert_eq!(
            partial
                .types
                .display(partial.definition_facts[&node.id].value.unwrap()),
            "{children: Array<Node>}"
        );
    }

    #[test]
    fn partial_type_evaluation_seals_multi_node_expr_components_and_dependents() {
        let partial = analyze_partial_types(
            "recursive-expr.telora",
            "type CallNode = struct {args: Array(Expr)};\
             type BinNode = struct {left: Expr, right: Expr};\
             type Expr = enum {'Literal(Int), 'Call(CallNode), 'Bin(BinNode)};\
             type Plan(A) = struct {root: Expr, value: A};\
             def render: Fn(Expr) -> String = fn(expr) { \"ok\" };\
             def transform: for(A) Fn(Plan(A)) -> String = fn(plan) { render(plan.root) };\
             def duplicate: Fn(Array(Expr)) -> Array(Expr) = fn(items) { items };\
             0",
            Quota::with_fuel(1_000),
        );
        let definition = |name: &str| {
            partial
                .hir
                .definitions()
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .id
        };
        for name in ["CallNode", "BinNode", "Expr", "Plan"] {
            assert_eq!(
                partial.definition_facts[&definition(name)].state,
                FactState::Known,
                "{name}"
            );
        }
        assert!(partial.diagnostics.is_empty());
        let plan = partial
            .definition_schemes
            .get(&definition("Plan"))
            .expect("dependent family keeps its scheme");
        assert_eq!(plan.display_name(), "for(A) Fn(TypeOf(A)) -> TypeOf(Plan)");
        assert!(
            !plan.display_name().contains("Any"),
            "{}",
            plan.display_name()
        );
    }

    #[test]
    fn partial_type_evaluation_rejects_recursive_aliases_and_families() {
        for (name, source) in [
            ("alias", "type Left = Right; type Right = Left; 0"),
            ("family", "type Loop(A) = struct {next: Loop(A)}; 0"),
        ] {
            let partial = analyze_partial_types(
                &format!("recursive-{name}.telora"),
                source,
                Quota::with_fuel(100),
            );
            assert!(partial.definition_facts.values().all(|fact| {
                fact.state == FactState::Incomputable(IncomputableReason::CyclicEvaluation)
            }));
            assert!(partial.diagnostics.iter().all(|diagnostic| {
                diagnostic.message.contains("cannot be partially evaluated")
            }));
        }
    }

    #[test]
    fn partial_type_evaluation_accepts_explicit_linked_capabilities() {
        let mut vm = Vm::new();
        let bindings = BTreeMap::from([(
            "LinkedType".to_owned(),
            TypeDescriptor::Int.to_value(&mut vm),
        )]);
        let partial = analyze_partial_types_with_bindings(
            "linked.telora",
            "type Linked = LinkedType; 0",
            Quota::with_fuel(10),
            &bindings,
        );
        let linked = partial
            .hir
            .definitions()
            .iter()
            .find(|definition| definition.name == "Linked")
            .unwrap();
        let fact = &partial.definition_facts[&linked.id];
        assert_eq!(fact.state, FactState::Known);
        assert_eq!(partial.types.node(fact.value.unwrap()), &TypeNode::Int);
        assert!(partial.hir.references().iter().any(|reference| {
            reference.name == "LinkedType"
                && reference.resolution == crate::hir::HirResolution::External
        }));
    }

    #[test]
    fn tool_stage_respects_evaluation_fuel() {
        let error = analyze_source_with_fuel("test", "type Number = Array(Int); 0", 0).unwrap_err();
        assert!(error.message.contains("fuel"));
    }

    #[test]
    fn tool_expressions_share_one_module_account() {
        let error = analyze_source_with_quota(
            "test",
            "type First = Array(Int); type Second = Array(Int); 0",
            Quota::new(1, 1_000, u64::MAX),
        )
        .unwrap_err();
        assert!(error.message.contains("fuel"));
    }

    #[test]
    fn type_decorators_share_tool_fuel_and_report_the_decorator_origin() {
        let source = "let same = fn(ctx, rhs) { rhs }; @same type T = Int; 0";
        let exhausted = analyze_source_with_fuel("decorator.telora", source, 0).unwrap_err();
        assert!(exhausted.message.contains("fuel"));

        let invalid = analyze_source(
            "decorator.telora",
            "let invalid = fn(ctx, rhs) { 1 }; @invalid type T = Int; 0",
        )
        .unwrap_err();
        assert!(invalid.message.contains("invalid metadata"));
        let diagnostic = invalid.diagnostic.expect("located decorator diagnostic");
        assert_eq!(diagnostic.labels[0].location.range(), 34..42);
    }

    #[test]
    fn rejects_invalid_metadata_protocol() {
        let error = analyze_source("test", "type Broken = {kind: 'Unknown}; 0").unwrap_err();
        assert!(error.message.contains("unknown value"));

        let malformed = analyze_source(
            "test",
            "type Broken = {kind: 'WithAttributes, inner: Int, attributes: []}; 0",
        )
        .unwrap_err();
        assert!(malformed.message.contains("attributes must be a Dict"));
    }

    #[test]
    fn runtime_validation_uses_computed_metadata() {
        let accepted = crate::run_source(
            "test",
            "type User = struct {name: String, age: Int};\
             validate(User, {age: 36, name: \"Ada\"})",
            100_000,
        )
        .unwrap();
        assert!(matches!(
            accepted,
            Value::Tagged { tag, .. } if tag.name() == "Ok"
        ));

        let rejected = crate::run_source(
            "test",
            "type User = struct {name: String, age: Int};\
             validate(User, {age: \"old\", name: \"Ada\"})",
            100_000,
        )
        .unwrap();
        assert!(matches!(
            rejected,
            Value::Tagged { tag, .. } if tag.name() == "Err"
        ));

        let family = crate::run_source(
            "test",
            "type Box(A) = struct {value: A};\
             validate(Box(Int), {value: 42})",
            100_000,
        )
        .unwrap();
        assert!(matches!(
            family,
            Value::Tagged { tag, .. } if tag.name() == "Ok"
        ));
    }

    #[test]
    fn fail_requires_a_string_message_and_has_never_type() {
        let analysis = analyze_source("fail.telora", "fail!(\"bad\", 1)").unwrap();
        assert_eq!(analysis.display(analysis.result_type), "Never");

        let error = analyze_source("fail.telora", "fail!(2, 1)").unwrap_err();
        assert!(
            error.message.contains("Int") && error.message.contains("String"),
            "{}",
            error.message
        );
    }

    #[test]
    fn program_bytecode_retains_declared_ownership_metadata_and_explicit_witnesses() {
        let erased = crate::compile_source(
            "test",
            "type User = struct {name: String}; let user: User = {name: \"Ada\"}; user.name",
        )
        .unwrap();
        assert!(erased.constants().iter().any(is_type_metadata));

        let retained =
            crate::compile_source("test", "type User = struct {name: String}; User").unwrap();
        assert!(retained.constants().iter().any(is_type_metadata));
    }

    fn is_type_metadata(value: &Value) -> bool {
        TypeDescriptor::from_value(value).is_ok()
    }
}
