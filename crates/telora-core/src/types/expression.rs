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
        ExprKind::TypeAscription { value, target } => {
            infer_expr_with(target, environment, record);
            infer_expr_with(value, environment, record)
        }
        ExprKind::CheckedCast { value, target } => {
            infer_expr_with(value, environment, record);
            infer_expr_with(target, environment, record);
            TypeDescriptor::Any
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            infer_expr_with(namespace, environment, record);
            infer_expr_with(target, environment, record);
            infer_expr_with(value, environment, record);
            TypeDescriptor::Any
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
        ExprKind::TypeAscription { value, target } | ExprKind::CheckedCast { value, target } => {
            check_interpolations(value, environment, sources)?;
            check_interpolations(target, environment, sources)?;
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            check_interpolations(namespace, environment, sources)?;
            check_interpolations(target, environment, sources)?;
            check_interpolations(value, environment, sources)?;
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

