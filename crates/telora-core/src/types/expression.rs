fn infer_expr_recorded(
    expression: &Expr,
    environment: &HashMap<String, TypeDescriptor>,
    facts: &mut HashMap<crate::Location, TypeDescriptor>,
) -> Option<TypeDescriptor> {
    infer_expr_with(expression, environment, &mut |location, descriptor| {
        facts.insert(location, descriptor.clone());
    })
}

// This projection records only evidence available before strict inference.
// Missing evidence is not a type and cannot justify a compatibility decision.
fn infer_expr_with(
    expression: &Expr,
    environment: &HashMap<String, TypeDescriptor>,
    record: &mut impl FnMut(crate::Location, &TypeDescriptor),
) -> Option<TypeDescriptor> {
    let inferred = match &expression.value {
        ExprKind::Int(_) => Some(TypeDescriptor::Int),
        ExprKind::Float(_) => Some(TypeDescriptor::Float),
        ExprKind::String(_) => Some(TypeDescriptor::String),
        ExprKind::Bytes(_) => Some(TypeDescriptor::Bytes),
        // Provisional constructor evidence lets tool-stage contracts reject bad
        // variants before evaluation. Strict inference supplies the enum owner.
        ExprKind::Atom(name) => Some(TypeDescriptor::Atom(atom_from_name(name))),
        ExprKind::Variable(name) => environment.get(&name.value).cloned(),
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    infer_expr_with(expression, environment, record);
                }
            }
            Some(TypeDescriptor::String)
        }
        ExprKind::Array(items) => {
            let items = items.iter().map(|item| {
                if let ExprKind::Spread(operand) = &item.value {
                    match infer_expr_with(operand, environment, record) {
                        Some(TypeDescriptor::Array(item)) => Some(*item),
                        _ => None,
                    }
                } else {
                    infer_expr_with(item, environment, record)
                }
            }).collect::<Vec<_>>();
            items.into_iter().collect::<Option<Vec<_>>>()
                .and_then(|items| if items.is_empty() { Some(TypeDescriptor::Never) } else { common_type(items) })
                .map(|item| TypeDescriptor::Array(Box::new(item)))
        }
        ExprKind::Tuple(items) => {
            let mut types = Vec::new();
            let mut complete = true;
            for item in items {
                if let ExprKind::Spread(operand) = &item.value {
                    if let Some(TypeDescriptor::Tuple(items)) = infer_expr_with(operand, environment, record) {
                        types.extend(items);
                    } else {
                        complete = false;
                    }
                } else if let Some(ty) = infer_expr_with(item, environment, record) {
                    types.push(ty);
                } else {
                    complete = false;
                }
            }
            complete.then_some(TypeDescriptor::Tuple(types))
        }
        ExprKind::Dict(fields) if fields.iter().any(|field| field.value.name.is_none()) => {
            let items = fields.iter().map(|field| {
                if field.value.name.is_none() {
                    let ExprKind::Spread(operand) = &field.value.value.value else { return None; };
                    match infer_expr_with(operand, environment, record) {
                        Some(TypeDescriptor::Dict(item)) => Some(*item),
                        _ => None,
                    }
                } else {
                    infer_expr_with(&field.value.value, environment, record)
                }
            }).collect::<Vec<_>>();
            items.into_iter().collect::<Option<Vec<_>>>()
                .and_then(common_type).map(|item| TypeDescriptor::Dict(Box::new(item)))
        }
        ExprKind::Dict(fields) => {
            let fields = fields.iter().map(|field| {
                let ty = infer_expr_with(&field.value.value, environment, record);
                ty.map(|ty| (field.value.name.as_ref().expect("ordinary field has a name").value.clone(), ty))
            }).collect::<Vec<_>>();
            fields.into_iter().collect::<Option<BTreeMap<_, _>>>().map(TypeDescriptor::Struct)
        }
        ExprKind::Block(block) => infer_block_with(block, environment, record),
        ExprKind::Spread(operand) | ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } =>
            infer_expr_with(operand, environment, record),
        ExprKind::Return { value } => {
            infer_expr_with(value, environment, record);
            Some(TypeDescriptor::Never)
        }
        ExprKind::Panic { message } => {
            infer_expr_with(message, environment, record);
            Some(TypeDescriptor::Never)
        }
        ExprKind::Raise { message, subjects } => {
            infer_expr_with(message, environment, record);
            for subject in subjects { infer_expr_with(subject, environment, record); }
            Some(TypeDescriptor::Never)
        }
        ExprKind::Debug { value, .. } => infer_expr_with(value, environment, record),
        ExprKind::Binary { operator, left, right } => {
            let left = infer_expr_with(left, environment, record);
            let right = infer_expr_with(right, environment, record);
            match operator.value {
                BinaryOperator::LessThan | BinaryOperator::LessThanOrEqual
                | BinaryOperator::GreaterThan | BinaryOperator::GreaterThanOrEqual
                | BinaryOperator::Equal | BinaryOperator::NotEqual => Some(normalized_bool_descriptor()),
                BinaryOperator::StructUpdate if matches!(&left,
                    Some(TypeDescriptor::Declared(declared))
                        if matches!(declared.body.as_ref(), TypeDescriptor::Struct(_))) => left,
                _ if left == right => left,
                _ => None,
            }
        }
        ExprKind::FieldProjection { receiver, .. } => {
            infer_expr_with(receiver, environment, record);
            None
        }
        ExprKind::Field { receiver, field } => {
            match infer_expr_with(receiver, environment, record) {
                Some(TypeDescriptor::Struct(fields)) => fields.get(&field.value).cloned(),
                Some(TypeDescriptor::Dict(item)) => Some(*item),
                _ => None,
            }
        }
        ExprKind::Index { receiver, index } => {
            let receiver = infer_expr_with(receiver, environment, record);
            infer_expr_with(index, environment, record);
            match receiver {
                Some(TypeDescriptor::Array(item)) => Some(*item),
                _ => None,
            }
        }
        ExprKind::TupleProjection { receiver, index } => {
            match infer_expr_with(receiver, environment, record) {
                Some(TypeDescriptor::Tuple(items)) => items.get(index.value).cloned(),
                _ => None,
            }
        }
        ExprKind::TypeAscription { value, target } => {
            let actual = infer_expr_with(value, environment, record);
            match infer_expr_with(target, environment, record) {
                Some(TypeDescriptor::TypeOf(target)) => Some(*target),
                _ => actual,
            }
        }
        ExprKind::CheckedCast { value, target } => {
            infer_expr_with(value, environment, record);
            match infer_expr_with(target, environment, record) {
                Some(TypeDescriptor::TypeOf(target)) => Some(result_descriptor(*target, TypeDescriptor::String)),
                _ => None,
            }
        }
        ExprKind::DynProject { namespace, target, value } => {
            infer_expr_with(namespace, environment, record);
            infer_expr_with(value, environment, record);
            match infer_expr_with(target, environment, record) {
                Some(TypeDescriptor::TypeOf(target)) => Some(option_descriptor(*target)),
                _ => None,
            }
        }
        ExprKind::TypeApply { callee, arguments } => {
            infer_expr_with(callee, environment, record);
            for argument in arguments {
                if let TypeArgumentKind::Explicit(argument) = &argument.value {
                    infer_expr_with(argument, environment, record);
                }
            }
            None
        }
        ExprKind::Call { callee, arguments } => {
            let callee = infer_expr_with(callee, environment, record);
            let arguments = arguments.iter().map(|argument| infer_expr_with(argument, environment, record)).collect::<Vec<_>>();
            match callee {
                Some(TypeDescriptor::Function { result, .. }) => {
                    let mut parameters = Vec::new();
                    collect_bound_parameters(&result, &mut parameters);
                    parameters.is_empty().then_some(*result)
                }
                Some(TypeDescriptor::Atom(tag)) if arguments.len() == 1 =>
                    arguments.into_iter().next().flatten().map(|payload| TypeDescriptor::Tagged { tag, payload: Box::new(payload) }),
                _ => None,
            }
        }
        ExprKind::Interpreter { elaboration, .. } => infer_expr_with(elaboration, environment, record),
        ExprKind::Closure { parameters, result_annotation, body } => {
            let mut closure_environment = environment.clone();
            let parameters = parameters.iter().map(|parameter| {
                let ty = parameter.annotation.as_ref().and_then(|annotation| {
                    match infer_expr_with(annotation, environment, record) {
                        Some(TypeDescriptor::TypeOf(ty)) => Some(*ty),
                        _ => None,
                    }
                });
                set_projected_type(&mut closure_environment, &parameter.name.value, ty.clone());
                ty
            }).collect::<Vec<_>>();
            if let Some(annotation) = result_annotation { infer_expr_with(annotation, environment, record); }
            let result = infer_block_with(body, &closure_environment, record);
            parameters.into_iter().collect::<Option<Vec<_>>>().zip(result)
                .map(|(parameters, result)| TypeDescriptor::Function { parameters, result: Box::new(result) })
        }
        ExprKind::If { condition, then_branch, else_branch } => {
            infer_expr_with(condition, environment, record);
            let left = infer_block_with(then_branch, environment, record);
            let right = infer_block_with(else_branch, environment, record);
            left.zip(right).map(|(left, right)| join_types(left, right))
        }
        ExprKind::IfLet { pattern, value, then_branch, else_branch } => {
            infer_expr_with(value, environment, record);
            let mut then_environment = environment.clone();
            clear_pattern_types(pattern, &mut then_environment);
            let left = infer_block_with(then_branch, &then_environment, record);
            let right = infer_block_with(else_branch, environment, record);
            left.zip(right).map(|(left, right)| join_types(left, right))
        }
        ExprKind::LetElse { pattern, value, else_branch, body } => {
            infer_expr_with(value, environment, record);
            infer_block_with(else_branch, environment, record);
            let mut body_environment = environment.clone();
            clear_pattern_types(pattern, &mut body_environment);
            infer_block_with(body, &body_environment, record)
        }
        ExprKind::Match { value, arms } => {
            infer_expr_with(value, environment, record);
            let arms = arms.iter().map(|arm| {
                let mut arm_environment = environment.clone();
                clear_pattern_types(&arm.value.pattern, &mut arm_environment);
                if let Some(guard) = &arm.value.guard { infer_expr_with(guard, &arm_environment, record); }
                infer_expr_with(&arm.value.value, &arm_environment, record)
            }).collect::<Vec<_>>();
            arms.into_iter().collect::<Option<Vec<_>>>().map(pending_alternatives)
        }
    };
    if let Some(inferred) = &inferred { record(expression.location, inferred); }
    inferred
}

fn set_projected_type(environment: &mut HashMap<String, TypeDescriptor>, name: &str, ty: Option<TypeDescriptor>) {
    if let Some(ty) = ty { environment.insert(name.to_owned(), ty); }
    else { environment.remove(name); }
}

fn infer_block_with(
    block: &Block,
    environment: &HashMap<String, TypeDescriptor>,
    record: &mut impl FnMut(crate::Location, &TypeDescriptor),
) -> Option<TypeDescriptor> {
    let mut environment = environment.clone();
    for binding in &block.value.bindings {
        environment.remove(&binding.value.name.value);
    }
    for binding in &block.value.bindings {
        if let Some(annotation) = &binding.value.annotation { infer_expr_with(annotation, &environment, record); }
        let inferred = infer_expr_with(&binding.value.value, &environment, record);
        if matches!(binding.value.kind, BindingKind::Let | BindingKind::Def | BindingKind::Import) {
            set_projected_type(&mut environment, &binding.value.name.value, inferred);
        }
    }
    infer_expr_with(&block.value.result, &environment, record)
}

fn clear_pattern_types(pattern: &Pattern, environment: &mut HashMap<String, TypeDescriptor>) {
    match &pattern.value {
        crate::ast::PatternKind::Binding(name) => { environment.remove(&name.value); }
        crate::ast::PatternKind::Tagged { payload, .. } => clear_pattern_types(payload, environment),
        crate::ast::PatternKind::Tuple(items) => {
            for item in items { clear_pattern_types(item, environment); }
        }
        crate::ast::PatternKind::Struct(fields) => {
            for field in fields { clear_pattern_types(&field.pattern, environment); }
        }
        _ => {}
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
