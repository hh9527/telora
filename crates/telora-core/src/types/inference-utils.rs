fn contains_standalone_sum(ty: &TypeDescriptor) -> bool {
    match ty {
        TypeDescriptor::AtomValue | TypeDescriptor::Atom(_) | TypeDescriptor::Tagged { .. } => true,
        TypeDescriptor::Newtype(item) | TypeDescriptor::Array(item) | TypeDescriptor::Dict(item) | TypeDescriptor::TypeOf(item) => {
            contains_standalone_sum(item)
        }
        TypeDescriptor::Declared(declared) => {
            declared.id.arguments().iter().any(contains_standalone_sum)
                || contains_standalone_sum(&declared.body)
        }
        TypeDescriptor::Tuple(items) | TypeDescriptor::PendingAlternatives(items) => {
            items.iter().any(contains_standalone_sum)
        }
        TypeDescriptor::Struct(fields) => fields.values().any(contains_standalone_sum),
        TypeDescriptor::Enum(variants) => variants.values().flatten().any(|ty| contains_standalone_sum(ty)),
        TypeDescriptor::Function { parameters, result } => {
            parameters.iter().any(contains_standalone_sum) || contains_standalone_sum(result)
        }
        _ => false,
    }
}

fn contains_type_variable(ty: &TypeDescriptor) -> bool {
    match ty {
        TypeDescriptor::Inference(_) => true,
        TypeDescriptor::Bound(_) => false,
        TypeDescriptor::Declared(declared) => {
            declared.id.arguments().iter().any(contains_type_variable)
                || contains_type_variable(&declared.body)
        }
        TypeDescriptor::Newtype(item) | TypeDescriptor::Array(item) => contains_type_variable(item),
        TypeDescriptor::Dict(item) => contains_type_variable(item),
        TypeDescriptor::TypeOf(instance) => contains_type_variable(instance),
        TypeDescriptor::Tagged { payload, .. } => contains_type_variable(payload),
        TypeDescriptor::Tuple(items) | TypeDescriptor::PendingAlternatives(items) => {
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
