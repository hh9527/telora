#[cfg(test)]
mod projection_tests {
    use super::*;

    #[test]
    fn root_projection_does_not_read_irrelevant_operand_types() {
        struct Inputs { function: TypeDescriptor, array: TypeDescriptor }
        impl TypeEnvironment for Inputs {
            fn get(&self, name: &str) -> Option<&TypeDescriptor> {
                match name {
                    "f" => Some(&self.function),
                    "items" => Some(&self.array),
                    _ => panic!("root projection unnecessarily read {name}"),
                }
            }
        }
        let environment = Inputs {
            function: TypeDescriptor::Function { parameters: vec![TypeDescriptor::Int], result: Box::new(TypeDescriptor::Int) },
            array: TypeDescriptor::Array(Box::new(TypeDescriptor::Int)),
        };
        for text in ["f(unneeded)", "items[unneeded]", "if unneeded { 1 } else { 2 }"] {
            let mut sources = SourceDatabase::default();
            let source = sources.add("root-projection", text);
            let program = parse_registered(&sources, source).program.unwrap();
            assert_eq!(infer_expr_projection(&program.value.body.value.result, &environment), Some(TypeDescriptor::Int));
        }
        let mut sources = SourceDatabase::default();
        let source = sources.add("untyped-closure-projection", "fn(x) { unneeded }");
        let program = parse_registered(&sources, source).program.unwrap();
        assert_eq!(infer_expr_projection(&program.value.body.value.result, &environment), None);
    }

    #[test]
    fn complete_solver_still_rejects_arguments_with_projectable_call_results() {
        let error = analyze_source("argument-check", r#"
            def f: Fn(Int) -> Int = fn(x) { x };
            def result = f("wrong");
            result
        "#).unwrap_err();
        assert!(error.message.contains("Int") && error.message.contains("String"), "{}", error.message);
    }
}

// This projection computes only the requested root type before strict inference.
// Missing evidence is not a type and cannot justify a compatibility decision.
fn infer_expr_projection(
    expression: &Expr,
    environment: &dyn TypeEnvironment,
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
        ExprKind::InterpolatedString(_) => Some(TypeDescriptor::String),
        ExprKind::Array(items) => {
            let items = items.iter().map(|item| {
                if let ExprKind::Spread(operand) = &item.value {
                    match infer_expr_projection(operand, environment) {
                        Some(TypeDescriptor::Array(item)) => Some(*item),
                        _ => None,
                    }
                } else {
                    infer_expr_projection(item, environment)
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
                    if let Some(TypeDescriptor::Tuple(items)) = infer_expr_projection(operand, environment) {
                        types.extend(items);
                    } else {
                        complete = false;
                    }
                } else if let Some(ty) = infer_expr_projection(item, environment) {
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
                    match infer_expr_projection(operand, environment) {
                        Some(TypeDescriptor::Dict(item)) => Some(*item),
                        _ => None,
                    }
                } else {
                    infer_expr_projection(&field.value.value, environment)
                }
            }).collect::<Vec<_>>();
            items.into_iter().collect::<Option<Vec<_>>>()
                .and_then(common_type).map(|item| TypeDescriptor::Dict(Box::new(item)))
        }
        ExprKind::Dict(fields) => {
            let fields = fields.iter().map(|field| {
                let ty = infer_expr_projection(&field.value.value, environment);
                ty.map(|ty| (field.value.name.as_ref().expect("ordinary field has a name").value.clone(), ty))
            }).collect::<Vec<_>>();
            fields.into_iter().collect::<Option<BTreeMap<_, _>>>().map(TypeDescriptor::Struct)
        }
        ExprKind::Block(block) => infer_block_projection(block, environment),
        ExprKind::TypeSyntax(operand) | ExprKind::TypeMetadata(operand) => {
            match infer_expr_projection(operand, environment) {
                Some(TypeDescriptor::Type) => None,
                inferred => inferred,
            }
        }
        ExprKind::Spread(operand) | ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } =>
            infer_expr_projection(operand, environment),
        ExprKind::Return { .. } | ExprKind::Panic { .. } => Some(TypeDescriptor::Never),
        ExprKind::Raise { action, .. } => {
            match action {
                crate::ast::BlameAction::Build => Some(TypeDescriptor::Opaque(crate::core::blame_native_type())),
                crate::ast::BlameAction::Warn => None,
                _ => Some(TypeDescriptor::Never),
            }
        }
        ExprKind::Debug { value, .. } => infer_expr_projection(value, environment),
        ExprKind::Binary { operator, left, right } => {
            if matches!(operator.value,
                BinaryOperator::LessThan | BinaryOperator::LessThanOrEqual
                | BinaryOperator::GreaterThan | BinaryOperator::GreaterThanOrEqual
                | BinaryOperator::Equal | BinaryOperator::NotEqual)
            {
                return Some(normalized_bool_descriptor());
            }
            let left = infer_expr_projection(left, environment);
            let right = infer_expr_projection(right, environment);
            match operator.value {
                BinaryOperator::StructUpdate if matches!(&left,
                    Some(TypeDescriptor::Declared(declared))
                        if matches!(declared.body.as_ref(), TypeDescriptor::Struct(_))) => left,
                _ if left == right => left,
                _ => None,
            }
        }
        ExprKind::FieldProjection { .. } => None,
        ExprKind::Field { receiver, field } => {
            match infer_expr_projection(receiver, environment) {
                Some(TypeDescriptor::Struct(fields)) => fields.get(&field.value).cloned(),
                Some(TypeDescriptor::Dict(item)) => Some(*item),
                _ => None,
            }
        }
        ExprKind::Index { receiver, .. } => {
            let receiver = infer_expr_projection(receiver, environment);
            match receiver {
                Some(TypeDescriptor::Array(item)) => Some(*item),
                _ => None,
            }
        }
        ExprKind::TupleProjection { receiver, index } => {
            match infer_expr_projection(receiver, environment) {
                Some(TypeDescriptor::Declared(declared)) if index.value == 0 => match declared.body.as_ref() {
                    TypeDescriptor::Newtype(payload) => Some(payload.as_ref().clone()),
                    _ => None,
                },
                Some(TypeDescriptor::Tuple(items)) => items.get(index.value).cloned(),
                _ => None,
            }
        }
        ExprKind::TypeAscription { value, target } => {
            match infer_expr_projection(target, environment) {
                Some(TypeDescriptor::TypeOf(target)) => Some(*target),
                _ => infer_expr_projection(value, environment),
            }
        }
        ExprKind::CheckedCast { target, .. } => {
            match infer_expr_projection(target, environment) {
                Some(TypeDescriptor::TypeOf(target)) => Some(result_descriptor(*target, TypeDescriptor::String)),
                _ => None,
            }
        }
        ExprKind::TypeApply { .. } => None,
        ExprKind::Call { callee, arguments } => {
            let callee = infer_expr_projection(callee, environment);
            match callee {
                Some(TypeDescriptor::Function { result, .. }) => {
                    let mut parameters = Vec::new();
                    collect_bound_parameters(&result, &mut parameters);
                    parameters.is_empty().then_some(*result)
                }
                Some(TypeDescriptor::Atom(tag)) if arguments.len() == 1 =>
                    infer_expr_projection(&arguments[0], environment).map(|payload| TypeDescriptor::Tagged { tag, payload: Box::new(payload) }),
                _ => None,
            }
        }
        ExprKind::Interpreter { elaboration, .. } => infer_expr_projection(elaboration, environment),
        ExprKind::Closure { parameters, body, .. } => {
            let mut closure_environment = ScopedTypeEnvironment::new(environment);
            let parameters = parameters.iter().map(|parameter| {
                let ty = parameter.annotation.as_ref().and_then(|annotation| {
                    match infer_expr_projection(annotation, environment) {
                        Some(TypeDescriptor::TypeOf(ty)) => Some(*ty),
                        _ => None,
                    }
                });
                set_projected_type(&mut closure_environment, &parameter.name.value, ty.clone());
                ty
            }).collect::<Option<Vec<_>>>()?;
            let result = infer_block_projection(body, &closure_environment)?;
            Some(TypeDescriptor::Function { parameters, result: Box::new(result) })
        }
        ExprKind::If { then_branch, else_branch, .. } => {
            let left = infer_block_projection(then_branch, environment);
            let right = infer_block_projection(else_branch, environment);
            left.zip(right).map(|(left, right)| join_types(left, right))
        }
        ExprKind::IfLet { pattern, then_branch, else_branch, .. } => {
            let mut then_environment = ScopedTypeEnvironment::new(environment);
            clear_pattern_types(pattern, &mut then_environment);
            let left = infer_block_projection(then_branch, &then_environment);
            let right = infer_block_projection(else_branch, environment);
            left.zip(right).map(|(left, right)| join_types(left, right))
        }
        ExprKind::LetElse { pattern, body, .. } => {
            let mut body_environment = ScopedTypeEnvironment::new(environment);
            clear_pattern_types(pattern, &mut body_environment);
            infer_block_projection(body, &body_environment)
        }
        ExprKind::Match { arms, .. } => {
            let arms = arms.iter().map(|arm| {
                let mut arm_environment = ScopedTypeEnvironment::new(environment);
                clear_pattern_types(&arm.value.pattern, &mut arm_environment);
                infer_expr_projection(&arm.value.value, &arm_environment)
            }).collect::<Vec<_>>();
            arms.into_iter().collect::<Option<Vec<_>>>().map(pending_alternatives)
        }
    };
    inferred
}

fn set_projected_type(environment: &mut dyn MutableTypeEnvironment, name: &str, ty: Option<TypeDescriptor>) {
    if let Some(ty) = ty { environment.insert(name.to_owned(), ty); }
    else { environment.remove(name); }
}

fn infer_block_projection(
    block: &Block,
    environment: &dyn TypeEnvironment,
) -> Option<TypeDescriptor> {
    let mut environment = ScopedTypeEnvironment::new(environment);
    let mut diverges = false;
    for binding in &block.value.bindings {
        environment.remove(&binding.value.name.value);
    }
    for binding in &block.value.bindings {
        let inferred = infer_expr_projection(&binding.value.value, &environment);
        diverges |= matches!(inferred, Some(TypeDescriptor::Never));
        if matches!(binding.value.kind, BindingKind::Let | BindingKind::Def | BindingKind::Import) {
            set_projected_type(&mut environment, &binding.value.name.value, inferred);
        }
    }
    let result = infer_expr_projection(&block.value.result, &environment);
    if diverges { Some(TypeDescriptor::Never) } else { result }
}

fn clear_pattern_types(pattern: &Pattern, environment: &mut dyn MutableTypeEnvironment) {
    match &pattern.value {
        crate::ast::PatternKind::Binding(name) => { environment.remove(&name.value); }
        crate::ast::PatternKind::Tagged { payload, .. } | crate::ast::PatternKind::Constructor { payload: Some(payload), .. } => clear_pattern_types(payload, environment),
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
