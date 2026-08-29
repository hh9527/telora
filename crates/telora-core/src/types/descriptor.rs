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
pub struct TypeParameter {
    pub id: TypeParameterId,
    pub name: String,
    pub location: crate::Location,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeScheme {
    pub parameters: Vec<TypeParameter>,
    pub constraints: Vec<TypeConstraint>,
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
            let constraints = self
                .constraints
                .iter()
                .fold(BTreeMap::<TypeParameterId, Vec<String>>::new(), |mut result, item| {
                    result
                        .entry(item.parameter)
                        .or_default()
                        .push(item.capability.display_name());
                    result
                });
            format!(
                "for({}) {body}",
                self.parameters
                    .iter()
                    .map(|parameter| {
                        constraints.get(&parameter.id).map_or_else(
                            || parameter.name.clone(),
                            |items| format!("{}: {}", parameter.name, items.join(" + ")),
                        )
                    })
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
    pub traits: BTreeMap<String, crate::TraitId>,
    pub trait_implementations: Vec<TraitImplementation>,
    pub type_properties: Vec<TypePropertyEvidence>,
    pub(crate) display_trait: Option<crate::TraitId>,
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
                            constraints: scheme.constraints.clone(),
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
            traits: self.traits.clone(),
            trait_implementations: self.trait_implementations.clone(),
            type_properties: self.type_properties.clone(),
            display_trait: self.display_trait,
            type_family_templates: self
                .type_family_templates
                .iter()
                .map(|(name, family)| {
                    (
                        name.clone(),
                        TypeFamilyTemplate {
                            parameters: family.parameters.clone(),
                            template: family.template,
                            root: family.root,
                            rebuild_at_runtime: family.rebuild_at_runtime,
                            constructor: family.constructor.clone(),
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
    template: PersistentValue,
    root: PersistentValue,
    rebuild_at_runtime: bool,
    constructor: Option<NominalTypeConstructor>,
}

#[derive(Clone, Debug)]
pub(crate) struct NominalTypeConstructor {
    pub(crate) id: crate::TypeConstructorId,
    pub(crate) name: String,
}

impl TypeFamilyTemplate {
    pub(crate) fn template(&self) -> PersistentValue {
        self.template
    }

    pub(crate) fn root(&self) -> PersistentValue {
        self.root
    }

    pub(crate) fn rebuild_at_runtime(&self) -> bool {
        self.rebuild_at_runtime
    }

    pub(crate) fn constructor(&self) -> Option<&NominalTypeConstructor> {
        self.constructor.as_ref()
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
    Any,
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

/// Stable identity for a type expression before all type-family parameters
/// have become concrete `TypeId`s. Nominal references use constructor IDs;
/// source names are deliberately not representable here.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum TypeExprId {
    Bound(u32),
    Declared(crate::TypeConstructorId, Box<[TypeExprId]>),
    Inference(u32),
    Any,
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
    Dict(Box<TypeExprId>),
    Tagged(String, Box<TypeExprId>),
    Tuple(Box<[TypeExprId]>),
    Struct(Box<[(String, TypeExprId)]>),
    Enum(Box<[(String, Option<TypeExprId>)]>),
    Union(Box<[TypeExprId]>),
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
            TypeDescriptor::Any => Self::Any,
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
            TypeDescriptor::Union(variants) => Self::Union(
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
            Self::Any => "Any".into(),
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
        TypeDescriptor::AtomValue => "Atom".into(),
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
