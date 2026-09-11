fn display_named_type(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
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

impl InferenceVariableId {
    const fn index(self) -> u32 {
        self.0
    }
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
    Never,
    Type,
    Dyn,
    TypeOf(Box<TypeDescriptor>),
    Int,
    Float,
    String,
    Bytes,
    AtomValue,
    Opaque(crate::NativeType),
    Atom(Atom),
    Array(Box<TypeDescriptor>),
    Dict(Box<TypeDescriptor>),
    Tagged {
        tag: Atom,
        payload: Box<TypeDescriptor>,
    },
    Tuple(Vec<TypeDescriptor>),
    Newtype(Box<TypeDescriptor>),
    Struct(BTreeMap<String, TypeDescriptor>),
    Enum(BTreeMap<String, Option<Box<TypeDescriptor>>>),
    /// Temporary inference candidates, never a published or runtime type.
    PendingAlternatives(Vec<TypeDescriptor>),
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

/// Stable identity for a type expression before all type-family parameters
/// have become concrete `TypeId`s. Nominal references use constructor IDs;
/// source names are deliberately not representable here.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum TypeExprId {
    Bound(u32),
    Declared(crate::TypeConstructorId, Box<[TypeExprId]>),
    Inference(u32),
    Never,
    Type,
    Dyn,
    TypeOf(Box<TypeExprId>),
    Int,
    Float,
    String,
    Bytes,
    AtomValue,
    Opaque(crate::value::NativeTypeId),
    Atom(String),
    Array(Box<TypeExprId>),
    Newtype(Box<TypeExprId>),
    Dict(Box<TypeExprId>),
    Tagged(String, Box<TypeExprId>),
    Tuple(Box<[TypeExprId]>),
    Struct(Box<[(String, TypeExprId)]>),
    Enum(Box<[(String, Option<TypeExprId>)]>),
    PendingAlternatives(Box<[TypeExprId]>),
    Function {
        parameters: Box<[TypeExprId]>,
        result: Box<TypeExprId>,
    },
}

impl TypeExprId {
    pub(crate) fn from_descriptor(descriptor: &TypeDescriptor) -> Self {
        match descriptor {
            TypeDescriptor::Bound(parameter) => Self::Bound(parameter.index()),
            TypeDescriptor::Named(name) => {
                panic!("unresolved named type {name:?} cannot participate in nominal identity")
            }
            TypeDescriptor::Declared(declared) => Self::Declared(
                declared.id.constructor(),
                declared
                    .id
                    .arguments()
                    .iter()
                    .map(Self::from_descriptor)
                    .collect::<Vec<_>>()
                    .into(),
            ),
            TypeDescriptor::Inference(variable) => Self::Inference(variable.index()),
            TypeDescriptor::Never => Self::Never,
            TypeDescriptor::Type => Self::Type,
            TypeDescriptor::Dyn => Self::Dyn,
            TypeDescriptor::TypeOf(inner) => Self::TypeOf(Box::new(Self::from_descriptor(inner))),
            TypeDescriptor::Int => Self::Int,
            TypeDescriptor::Float => Self::Float,
            TypeDescriptor::String => Self::String,
            TypeDescriptor::Bytes => Self::Bytes,
            TypeDescriptor::AtomValue => Self::AtomValue,
            TypeDescriptor::Opaque(native) => Self::Opaque(native.id()),
            TypeDescriptor::Atom(atom) => Self::Atom(atom.name().to_owned()),
            TypeDescriptor::Array(item) => Self::Array(Box::new(Self::from_descriptor(item))),
            TypeDescriptor::Newtype(item) => Self::Newtype(Box::new(Self::from_descriptor(item))),
            TypeDescriptor::Dict(item) => Self::Dict(Box::new(Self::from_descriptor(item))),
            TypeDescriptor::Tagged { tag, payload } => Self::Tagged(
                tag.name().to_owned(),
                Box::new(Self::from_descriptor(payload)),
            ),
            TypeDescriptor::Tuple(items) => Self::Tuple(
                items
                    .iter()
                    .map(Self::from_descriptor)
                    .collect::<Vec<_>>()
                    .into(),
            ),
            TypeDescriptor::Struct(fields) => Self::Struct(
                fields
                    .iter()
                    .map(|(name, field)| (name.clone(), Self::from_descriptor(field)))
                    .collect::<Vec<_>>()
                    .into(),
            ),
            TypeDescriptor::Enum(variants) => Self::Enum(
                variants
                    .iter()
                    .map(|(name, payload)| {
                        (name.clone(), payload.as_deref().map(Self::from_descriptor))
                    })
                    .collect::<Vec<_>>()
                    .into(),
            ),
            TypeDescriptor::PendingAlternatives(variants) => Self::PendingAlternatives(
                variants
                    .iter()
                    .map(Self::from_descriptor)
                    .collect::<Vec<_>>()
                    .into(),
            ),
            TypeDescriptor::Function { parameters, result } => Self::Function {
                parameters: parameters
                    .iter()
                    .map(Self::from_descriptor)
                    .collect::<Vec<_>>()
                    .into(),
                result: Box::new(Self::from_descriptor(result)),
            },
        }
    }
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
    pub fn display_name(&self) -> String {
        match self {
            Self::Bound(parameter) => format!("T{}", parameter.0),
            Self::Named(name) => display_named_type(name).to_owned(),
            Self::Declared(declared) => declared.name.clone(),
            Self::Inference(variable) => format!("?{}", variable.0),
            Self::Never => "Never".into(),
            Self::Type => "Type".into(),
            Self::Dyn => "Dyn".into(),
            Self::TypeOf(instance) => format!("TypeOf({})", instance.display_name()),
            Self::Int => "Int".into(),
            Self::Float => "Float".into(),
            Self::String => "String".into(),
            Self::Bytes => "Bytes".into(),
            Self::AtomValue => "Atom".into(),
            Self::Opaque(native_type) => format!("opaque({})", native_type.qualified_name()),
            Self::Atom(atom) => format!("'{}", atom.name()),
            Self::Array(item) => format!("Array<{}>", item.display_name()),
            Self::Newtype(item) => format!("struct({})", item.display_name()),
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
            Self::PendingAlternatives(variants) => variants
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
