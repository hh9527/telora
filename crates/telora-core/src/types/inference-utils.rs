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
    for constraint in &scheme.constraints {
        if !declared.contains(&constraint.parameter) {
            return Err(format!(
                "constraint references unbound parameter T{}",
                constraint.parameter.index()
            ));
        }
        if let TypeCapability::Property(property) = &constraint.capability {
            if contains_type_variable(property) {
                return Err(format!(
                    "property constraint contains unresolved {}",
                    property.display_name()
                ));
            }
            let mut property_parameters = Vec::new();
            collect_bound_parameters(property, &mut property_parameters);
            if let Some(parameter) = property_parameters
                .into_iter()
                .find(|parameter| !declared.contains(parameter))
            {
                return Err(format!(
                    "property constraint references unbound parameter T{}",
                    parameter.index()
                ));
            }
        }
    }
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

fn contains_exposed_type_variable(ty: &TypeDescriptor) -> bool {
    match ty {
        TypeDescriptor::Inference(_) => true,
        TypeDescriptor::Declared(declared) => declared
            .id
            .arguments()
            .iter()
            .any(contains_exposed_type_variable),
        TypeDescriptor::Array(item)
        | TypeDescriptor::Dict(item)
        | TypeDescriptor::TypeOf(item)
        | TypeDescriptor::Tagged { payload: item, .. } => contains_exposed_type_variable(item),
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            items.iter().any(contains_exposed_type_variable)
        }
        TypeDescriptor::Struct(fields) => fields.values().any(contains_exposed_type_variable),
        TypeDescriptor::Enum(variants) => variants
            .values()
            .flatten()
            .any(|payload| contains_exposed_type_variable(payload)),
        TypeDescriptor::Function { parameters, result } => {
            parameters.iter().any(contains_exposed_type_variable)
                || contains_exposed_type_variable(result)
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

fn same_nominal_head_with_erased_arguments(
    actual: &TypeDescriptor,
    expected: &TypeDescriptor,
) -> bool {
    let (TypeDescriptor::Declared(actual), TypeDescriptor::Declared(expected)) = (actual, expected)
    else {
        return false;
    };
    actual.id.has_same_head(&expected.id)
        && actual.id.arguments().len() == expected.id.arguments().len()
        && actual
            .id
            .arguments()
            .iter()
            .zip(expected.id.arguments())
            .all(|(actual, expected)| {
                assignable(
                    &erase_declared_identity(actual),
                    &erase_declared_identity(expected),
                )
            })
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

fn contains_any_descriptor(descriptor: &TypeDescriptor) -> bool {
    match descriptor {
        TypeDescriptor::Any => true,
        TypeDescriptor::Declared(declared) => contains_any_descriptor(&declared.body),
        TypeDescriptor::TypeOf(item)
        | TypeDescriptor::Array(item)
        | TypeDescriptor::Dict(item)
        | TypeDescriptor::Tagged { payload: item, .. } => contains_any_descriptor(item),
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            items.iter().any(contains_any_descriptor)
        }
        TypeDescriptor::Struct(fields) => fields.values().any(contains_any_descriptor),
        TypeDescriptor::Enum(variants) => variants
            .values()
            .flatten()
            .any(|payload| contains_any_descriptor(payload)),
        TypeDescriptor::Function { parameters, result } => {
            parameters.iter().any(contains_any_descriptor) || contains_any_descriptor(result)
        }
        TypeDescriptor::Named(_)
        | TypeDescriptor::Never
        | TypeDescriptor::Type
        | TypeDescriptor::Dyn
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Atom(_)
        | TypeDescriptor::Bound(_)
        | TypeDescriptor::Inference(_) => false,
    }
}

fn narrows_any(actual: &TypeDescriptor, expected: &TypeDescriptor) -> bool {
    if matches!(
        expected,
        TypeDescriptor::Any | TypeDescriptor::Inference(_) | TypeDescriptor::Bound(_)
    ) {
        return false;
    }
    if matches!(actual, TypeDescriptor::Any) {
        return true;
    }
    match (actual, expected) {
        (TypeDescriptor::Declared(actual), TypeDescriptor::Declared(expected))
            if actual.id == expected.id =>
        {
            actual
                .id
                .arguments()
                .iter()
                .zip(expected.id.arguments())
                .any(|(actual, expected)| narrows_any(actual, expected))
        }
        (TypeDescriptor::Declared(actual), expected) => narrows_any(&actual.body, expected),
        (actual, TypeDescriptor::Declared(expected)) => narrows_any(actual, &expected.body),
        (TypeDescriptor::Array(actual), TypeDescriptor::Array(expected))
        | (TypeDescriptor::Dict(actual), TypeDescriptor::Dict(expected))
        | (TypeDescriptor::TypeOf(actual), TypeDescriptor::TypeOf(expected)) => {
            narrows_any(actual, expected)
        }
        (
            TypeDescriptor::Tagged {
                payload: actual, ..
            },
            TypeDescriptor::Tagged {
                payload: expected, ..
            },
        ) => narrows_any(actual, expected),
        (TypeDescriptor::Tuple(actual), TypeDescriptor::Tuple(expected))
        | (TypeDescriptor::Union(actual), TypeDescriptor::Union(expected))
            if actual.len() == expected.len() =>
        {
            actual
                .iter()
                .zip(expected)
                .any(|(actual, expected)| narrows_any(actual, expected))
        }
        (TypeDescriptor::Struct(actual), TypeDescriptor::Struct(expected)) => {
            actual.iter().any(|(name, actual)| {
                expected
                    .get(name)
                    .is_some_and(|expected| narrows_any(actual, expected))
            })
        }
        (TypeDescriptor::Enum(actual), TypeDescriptor::Enum(expected)) => {
            actual.iter().any(|(name, actual)| {
                match (actual, expected.get(name).and_then(Option::as_deref)) {
                    (Some(actual), Some(expected)) => narrows_any(actual, expected),
                    _ => false,
                }
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
        ) if actual_parameters.len() == expected_parameters.len() => {
            actual_parameters
                .iter()
                .zip(expected_parameters)
                .any(|(actual, expected)| narrows_any(actual, expected))
                || narrows_any(actual_result, expected_result)
        }
        _ => false,
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
        ExprKind::TypeAscription { value, target } | ExprKind::CheckedCast { value, target } => {
            expression_references_names(value, names, bound)
                || expression_references_names(target, names, bound)
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            expression_references_names(namespace, names, bound)
                || expression_references_names(target, names, bound)
                || expression_references_names(value, names, bound)
        }
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

fn validate_export_references<'a, T>(
    program: &Program,
    prelude: impl Iterator<Item = &'a String>,
    external_values: &BTreeMap<String, T>,
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
