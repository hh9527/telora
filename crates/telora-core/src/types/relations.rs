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

pub(crate) fn type_identity_contains_bound_parameter(descriptor: &TypeDescriptor) -> bool {
    match descriptor {
        TypeDescriptor::Bound(_) => true,
        TypeDescriptor::Declared(declared) => declared
            .id
            .arguments()
            .iter()
            .any(type_identity_contains_bound_parameter),
        TypeDescriptor::Newtype(item) | TypeDescriptor::Array(item) | TypeDescriptor::Dict(item) | TypeDescriptor::TypeOf(item) => {
            type_identity_contains_bound_parameter(item)
        }
        TypeDescriptor::Tagged { payload, .. } => type_identity_contains_bound_parameter(payload),
        TypeDescriptor::Tuple(items) | TypeDescriptor::PendingAlternatives(items) => {
            items.iter().any(type_identity_contains_bound_parameter)
        }
        TypeDescriptor::Struct(fields) => {
            fields.values().any(type_identity_contains_bound_parameter)
        }
        TypeDescriptor::Enum(variants) => variants.values().any(|payload| {
            payload
                .as_deref()
                .is_some_and(type_identity_contains_bound_parameter)
        }),
        TypeDescriptor::Function { parameters, result } => {
            parameters
                .iter()
                .any(type_identity_contains_bound_parameter)
                || type_identity_contains_bound_parameter(result)
        }
        TypeDescriptor::Named(_)
        | TypeDescriptor::Inference(_)
        | TypeDescriptor::Never
        | TypeDescriptor::Type
        | TypeDescriptor::Dyn
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::AtomValue
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Atom(_) => false,
    }
}

pub(crate) fn type_identity_is_symbolic(descriptor: &TypeDescriptor) -> bool {
    match descriptor {
        TypeDescriptor::Bound(_) | TypeDescriptor::Named(_) | TypeDescriptor::Inference(_) => true,
        TypeDescriptor::Declared(declared) => declared
            .id
            .arguments()
            .iter()
            .any(type_identity_is_symbolic),
        TypeDescriptor::Newtype(item) | TypeDescriptor::Array(item) | TypeDescriptor::Dict(item) | TypeDescriptor::TypeOf(item) => {
            type_identity_is_symbolic(item)
        }
        TypeDescriptor::Tagged { payload, .. } => type_identity_is_symbolic(payload),
        TypeDescriptor::Tuple(items) | TypeDescriptor::PendingAlternatives(items) => {
            items.iter().any(type_identity_is_symbolic)
        }
        TypeDescriptor::Struct(fields) => fields.values().any(type_identity_is_symbolic),
        TypeDescriptor::Enum(variants) => variants
            .values()
            .any(|payload| payload.as_deref().is_some_and(type_identity_is_symbolic)),
        TypeDescriptor::Function { parameters, result } => {
            parameters.iter().any(type_identity_is_symbolic) || type_identity_is_symbolic(result)
        }
        TypeDescriptor::Never
        | TypeDescriptor::Type
        | TypeDescriptor::Dyn
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::AtomValue
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Atom(_) => false,
    }
}


fn contains_pending_alternatives(ty: &TypeDescriptor) -> bool {
    match ty {
        TypeDescriptor::PendingAlternatives(_) => true,
        TypeDescriptor::Newtype(item) | TypeDescriptor::Array(item) | TypeDescriptor::Dict(item)
        | TypeDescriptor::TypeOf(item) | TypeDescriptor::Tagged { payload: item, .. } => {
            contains_pending_alternatives(item)
        }
        TypeDescriptor::Tuple(items) => items.iter().any(contains_pending_alternatives),
        TypeDescriptor::Struct(fields) => fields.values().any(contains_pending_alternatives),
        TypeDescriptor::Enum(variants) => variants.values().flatten()
            .any(|ty| contains_pending_alternatives(ty)),
        TypeDescriptor::Function { parameters, result } => {
            parameters.iter().any(contains_pending_alternatives)
                || contains_pending_alternatives(result)
        }
        TypeDescriptor::Declared(declared) => contains_pending_alternatives(&declared.body),
        _ => false,
    }
}

fn pending_alternatives(types: Vec<TypeDescriptor>) -> TypeDescriptor {
    fn flatten(ty: TypeDescriptor, flattened: &mut Vec<TypeDescriptor>) {
        match ty {
            TypeDescriptor::PendingAlternatives(variants) => {
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
        1 => flattened.pop().expect("one remaining candidate"),
        _ => TypeDescriptor::PendingAlternatives(flattened),
    }
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
