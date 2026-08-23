fn atom_constant(name: &str) -> Constant {
    let builtin = match name {
        "None" => Some(BuiltinAtom::None),
        "Some" => Some(BuiltinAtom::Some),
        "Ok" => Some(BuiltinAtom::Ok),
        "Err" => Some(BuiltinAtom::Err),
        "True" => Some(BuiltinAtom::True),
        "False" => Some(BuiltinAtom::False),
        _ => None,
    };
    Constant::Atom(match builtin {
        Some(builtin) => Atom::builtin(builtin),
        None => Atom::named(name),
    })
}

pub(crate) fn function_contract_arity(contract: &Expr) -> Option<u32> {
    let ExprKind::Call { callee, arguments } = &contract.value else {
        return None;
    };
    let ExprKind::Variable(name) = &callee.value else {
        return None;
    };
    if name.value != "Func" || arguments.len() != 2 {
        return None;
    }
    let ExprKind::Array(parameters) = &arguments[0].value else {
        return None;
    };
    u32::try_from(parameters.len()).ok()
}

fn free_block(block: &Block, bound: &mut HashSet<String>, free: &mut BTreeSet<String>) {
    for binding in &block.value.bindings {
        if matches!(binding.value.kind, BindingKind::Decl | BindingKind::Native) {
            bound.insert(binding.value.name.value.clone());
        }
    }
    for binding in &block.value.bindings {
        if !matches!(binding.value.kind, BindingKind::Decl | BindingKind::Native) {
            free_expr(&binding.value.value, bound, free);
        }
        bound.insert(binding.value.name.value.clone());
    }
    free_expr(&block.value.result, bound, free);
}

fn free_expr(expression: &Expr, bound: &HashSet<String>, free: &mut BTreeSet<String>) {
    match &expression.value {
        ExprKind::Variable(name) => {
            if !bound.contains(&name.value) {
                free.insert(name.value.clone());
            }
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                free_expr(item, bound, free);
            }
        }
        ExprKind::Spread(operand) => free_expr(operand, bound, free),
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    free_expr(expression, bound, free);
                }
            }
        }
        ExprKind::Dict(fields) => {
            for field in fields {
                free_expr(&field.value.value, bound, free);
            }
        }
        ExprKind::Block(block) => {
            let mut inner = bound.clone();
            free_block(block, &mut inner, free);
        }
        ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } => {
            free_expr(operand, bound, free)
        }
        ExprKind::Return { value } => free_expr(value, bound, free),
        ExprKind::Panic { message } => free_expr(message, bound, free),
        ExprKind::Raise { error } => free_expr(error, bound, free),
        ExprKind::Debug { value, .. } => free_expr(value, bound, free),
        ExprKind::Binary { left, right, .. } => {
            free_expr(left, bound, free);
            free_expr(right, bound, free);
        }
        ExprKind::Field { receiver, .. } => free_expr(receiver, bound, free),
        ExprKind::Index { receiver, index } => {
            free_expr(receiver, bound, free);
            free_expr(index, bound, free);
        }
        ExprKind::TupleProjection { receiver, .. } => free_expr(receiver, bound, free),
        ExprKind::TypeAscription { value, .. } => free_expr(value, bound, free),
        ExprKind::CheckedCast { value, target } => {
            free.insert("\0telora_cast".to_owned());
            free_expr(target, bound, free);
            free_expr(value, bound, free);
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            free_expr(namespace, bound, free);
            free_expr(target, bound, free);
            free_expr(value, bound, free);
        }
        ExprKind::Call { callee, arguments } => {
            free_expr(callee, bound, free);
            for argument in arguments {
                free_expr(argument, bound, free);
            }
        }
        ExprKind::TypeApply { callee, .. } => free_expr(callee, bound, free),
        ExprKind::Interpreter { elaboration, .. } => free_expr(elaboration, bound, free),
        ExprKind::Closure {
            parameters, body, ..
        } => {
            let mut closure_bound = parameters
                .iter()
                .map(|parameter| parameter.name.value.clone())
                .collect::<HashSet<_>>();
            let mut closure_free = BTreeSet::new();
            free_block(body, &mut closure_bound, &mut closure_free);
            for name in closure_free {
                if !bound.contains(&name) {
                    free.insert(name);
                }
            }
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            free_expr(condition, bound, free);
            let mut then_bound = bound.clone();
            free_block(then_branch, &mut then_bound, free);
            let mut else_bound = bound.clone();
            free_block(else_branch, &mut else_bound, free);
        }
        ExprKind::IfLet {
            pattern,
            value,
            then_branch,
            else_branch,
        } => {
            free_expr(value, bound, free);
            let mut then_bound = bound.clone();
            bind_pattern(pattern, &mut then_bound);
            free_block(then_branch, &mut then_bound, free);
            let mut else_bound = bound.clone();
            free_block(else_branch, &mut else_bound, free);
        }
        ExprKind::LetElse {
            pattern,
            value,
            else_branch,
            body,
        } => {
            free_expr(value, bound, free);
            let mut else_bound = bound.clone();
            free_block(else_branch, &mut else_bound, free);
            let mut body_bound = bound.clone();
            bind_pattern(pattern, &mut body_bound);
            free_block(body, &mut body_bound, free);
        }
        ExprKind::Match { value, arms } => {
            free_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                bind_pattern(&arm.value.pattern, &mut arm_bound);
                if let Some(guard) = &arm.value.guard {
                    free_expr(guard, &arm_bound, free);
                }
                free_expr(&arm.value.value, &arm_bound, free);
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_) => {}
    }
}

fn bind_pattern(pattern: &Pattern, bound: &mut HashSet<String>) {
    match &pattern.value {
        PatternKind::Binding(name) => {
            bound.insert(name.value.clone());
        }
        PatternKind::Tuple(items) => {
            for item in items {
                bind_pattern(item, bound);
            }
        }
        PatternKind::Tagged { payload, .. } => bind_pattern(payload, bound),
        PatternKind::Struct(fields) => {
            for field in fields {
                bind_pattern(&field.pattern, bound);
            }
        }
        PatternKind::Wildcard
        | PatternKind::Int(_)
        | PatternKind::Float(_)
        | PatternKind::String(_)
        | PatternKind::Atom(_) => {}
    }
}

fn collect_runtime_names_block(block: &Block, names: &mut HashSet<String>) {
    for binding in &block.value.bindings {
        if matches!(binding.value.kind, BindingKind::Let | BindingKind::Def) {
            collect_runtime_names(&binding.value.value, names);
        }
    }
    collect_runtime_names(&block.value.result, names);
}

fn collect_decorator_runtime_names(decorator: &crate::ast::Decorator, names: &mut HashSet<String>) {
    collect_runtime_names(&decorator.value.callee, names);
    for argument in &decorator.value.arguments {
        collect_runtime_names(argument, names);
    }
}

pub(crate) fn collect_runtime_names(expression: &Expr, names: &mut HashSet<String>) {
    match &expression.value {
        ExprKind::Variable(name) => {
            names.insert(name.value.clone());
        }
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                collect_runtime_names(item, names);
            }
        }
        ExprKind::Spread(operand) => collect_runtime_names(operand, names),
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &part.value {
                    collect_runtime_names(expression, names);
                }
            }
        }
        ExprKind::Dict(fields) => {
            for field in fields {
                for decorator in &field.value.decorators {
                    collect_decorator_runtime_names(decorator, names);
                }
                collect_runtime_names(&field.value.value, names);
            }
        }
        ExprKind::Block(block) => collect_runtime_names_block(block, names),
        ExprKind::Unary { operand, .. } | ExprKind::Propagate { operand } => {
            collect_runtime_names(operand, names)
        }
        ExprKind::Return { value } => collect_runtime_names(value, names),
        ExprKind::Panic { message } => collect_runtime_names(message, names),
        ExprKind::Raise { error } => collect_runtime_names(error, names),
        ExprKind::Debug { value, .. } => collect_runtime_names(value, names),
        ExprKind::Binary { left, right, .. } => {
            collect_runtime_names(left, names);
            collect_runtime_names(right, names);
        }
        ExprKind::Field { receiver, .. } => collect_runtime_names(receiver, names),
        ExprKind::Index { receiver, index } => {
            collect_runtime_names(receiver, names);
            collect_runtime_names(index, names);
        }
        ExprKind::TupleProjection { receiver, .. } => collect_runtime_names(receiver, names),
        ExprKind::TypeAscription { value, .. } => collect_runtime_names(value, names),
        ExprKind::CheckedCast { value, target } => {
            names.insert("\0telora_cast".to_owned());
            collect_runtime_names(target, names);
            collect_runtime_names(value, names);
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            collect_runtime_names(namespace, names);
            collect_runtime_names(target, names);
            collect_runtime_names(value, names);
        }
        ExprKind::Call { callee, arguments } => {
            collect_runtime_names(callee, names);
            for argument in arguments {
                collect_runtime_names(argument, names);
            }
        }
        ExprKind::TypeApply { callee, .. } => collect_runtime_names(callee, names),
        ExprKind::Interpreter { elaboration, .. } => {
            collect_runtime_names(elaboration, names);
        }
        ExprKind::Closure { body, .. } => collect_runtime_names_block(body, names),
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            collect_runtime_names(condition, names);
            collect_runtime_names_block(then_branch, names);
            collect_runtime_names_block(else_branch, names);
        }
        ExprKind::IfLet {
            value,
            then_branch,
            else_branch,
            ..
        } => {
            collect_runtime_names(value, names);
            collect_runtime_names_block(then_branch, names);
            collect_runtime_names_block(else_branch, names);
        }
        ExprKind::LetElse {
            value,
            else_branch,
            body,
            ..
        } => {
            collect_runtime_names(value, names);
            collect_runtime_names_block(else_branch, names);
            collect_runtime_names_block(body, names);
        }
        ExprKind::Match { value, arms } => {
            collect_runtime_names(value, names);
            for arm in arms {
                if let Some(guard) = &arm.value.guard {
                    collect_runtime_names(guard, names);
                }
                collect_runtime_names(&arm.value.value, names);
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_) => {}
    }
}

fn frontend_error(source_name: &str, message: impl Into<String>) -> FrontendError {
    FrontendError::new(
        source_name,
        SourceLocation {
            offset: 0,
            line: 1,
            column: 1,
        },
        message,
    )
}

