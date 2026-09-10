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
            if declared.id.constructor() == unchecked_type_constructor() {
                return unchecked_descriptor(arguments[0].clone());
            }
            TypeDescriptor::Declared(DeclaredTypeDescriptor {
                id: declared.id.reapply(&arguments),
                name: declared.name.clone(),
                body: Arc::new(body),
            })
        }
        TypeDescriptor::Array(item) => {
            TypeDescriptor::Array(Box::new(substitute_bound_parameters(item, replacements)))
        }
        TypeDescriptor::Newtype(item) => {
            TypeDescriptor::Newtype(Box::new(substitute_bound_parameters(item, replacements)))
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
        TypeDescriptor::PendingAlternatives(variants) => pending_alternatives(
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
