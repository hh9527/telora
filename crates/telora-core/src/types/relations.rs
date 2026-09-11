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
