use crate::Location;
use crate::ast::*;
use crate::types::ValueConstructor;
use std::collections::HashMap;

fn lower_pattern(pattern: &mut Pattern, constructors: &HashMap<Location, ValueConstructor>) {
    match &mut pattern.value {
        PatternKind::Binding(name) if constructors.contains_key(&name.location) => {
            pattern.value = PatternKind::Constructor {
                constructor: Box::new(located(ExprKind::Variable(name.clone()), name.location)),
                payload: None,
            };
        }
        PatternKind::Constructor {
            payload: Some(payload),
            ..
        }
        | PatternKind::Tagged { payload, .. } => {
            self::lower_pattern(payload, constructors);
        }
        PatternKind::Tuple(items) => {
            for item in items {
                self::lower_pattern(item, constructors);
            }
        }
        PatternKind::Struct(fields) => {
            for field in fields {
                self::lower_pattern(&mut field.pattern, constructors);
            }
        }
        _ => {}
    }
}
pub(crate) fn lower_block_constructor_patterns(
    block: &mut Block,
    constructors: &HashMap<Location, ValueConstructor>,
) {
    for binding in &mut block.value.bindings {
        for decorator in &mut binding.value.decorators {
            lower_constructor_patterns(&mut decorator.value.callee, constructors);
            for argument in &mut decorator.value.arguments {
                lower_constructor_patterns(argument, constructors);
            }
        }
        if let Some(annotation) = &mut binding.value.annotation {
            lower_constructor_patterns(annotation, constructors);
        }
        for bounds in &mut binding.value.type_parameter_bounds {
            for bound in bounds {
                lower_constructor_patterns(bound, constructors);
            }
        }
        lower_constructor_patterns(&mut binding.value.value, constructors);
    }
    lower_constructor_patterns(&mut block.value.result, constructors);
}
pub(crate) fn lower_constructor_patterns(
    expression: &mut Expr,
    constructors: &HashMap<Location, ValueConstructor>,
) {
    match &mut expression.value {
        ExprKind::Block(body) => lower_block_constructor_patterns(body, constructors),
        ExprKind::Array(items) | ExprKind::Tuple(items) => {
            for item in items {
                lower_constructor_patterns(item, constructors);
            }
        }
        ExprKind::InterpolatedString(parts) => {
            for part in parts {
                if let StringPartKind::Expression(expression) = &mut part.value {
                    lower_constructor_patterns(expression, constructors);
                }
            }
        }
        ExprKind::Dict(fields) => {
            for field in fields {
                for decorator in &mut field.value.decorators {
                    lower_constructor_patterns(&mut decorator.value.callee, constructors);
                    for argument in &mut decorator.value.arguments {
                        lower_constructor_patterns(argument, constructors);
                    }
                }
                lower_constructor_patterns(&mut field.value.value, constructors);
            }
        }
        ExprKind::Spread(value)
        | ExprKind::Unary { operand: value, .. }
        | ExprKind::Propagate { operand: value }
        | ExprKind::Return { value }
        | ExprKind::Panic { message: value }
        | ExprKind::Debug { value, .. }
        | ExprKind::Field {
            receiver: value, ..
        }
        | ExprKind::TupleProjection {
            receiver: value, ..
        }
        | ExprKind::FieldProjection {
            receiver: value, ..
        } => lower_constructor_patterns(value, constructors),
        ExprKind::Raise { message, subjects } => {
            lower_constructor_patterns(message, constructors);
            for subject in subjects {
                lower_constructor_patterns(subject, constructors);
            }
        }
        ExprKind::TypeAscription {
            value: left,
            target: right,
        }
        | ExprKind::CheckedCast {
            value: left,
            target: right,
        }
        | ExprKind::Binary { left, right, .. }
        | ExprKind::Index {
            receiver: left,
            index: right,
        }
        | ExprKind::Interpreter {
            operand: left,
            elaboration: right,
        } => {
            lower_constructor_patterns(left, constructors);
            lower_constructor_patterns(right, constructors);
        }
        ExprKind::DynProject {
            namespace,
            target,
            value,
        } => {
            lower_constructor_patterns(namespace, constructors);
            lower_constructor_patterns(target, constructors);
            lower_constructor_patterns(value, constructors);
        }
        ExprKind::Call { callee, arguments } => {
            lower_constructor_patterns(callee, constructors);
            for argument in arguments {
                lower_constructor_patterns(argument, constructors);
            }
        }
        ExprKind::TypeApply { callee, arguments } => {
            lower_constructor_patterns(callee, constructors);
            for argument in arguments {
                if let TypeArgumentKind::Explicit(argument) = &mut argument.value {
                    lower_constructor_patterns(argument, constructors);
                }
            }
        }
        ExprKind::Closure {
            parameters,
            result_annotation,
            body,
        } => {
            for annotation in parameters
                .iter_mut()
                .filter_map(|parameter| parameter.annotation.as_mut())
                .chain(result_annotation.as_deref_mut())
            {
                lower_constructor_patterns(annotation, constructors);
            }
            lower_block_constructor_patterns(body, constructors);
        }
        ExprKind::If {
            condition,
            then_branch,
            else_branch,
        } => {
            lower_constructor_patterns(condition, constructors);
            lower_block_constructor_patterns(then_branch, constructors);
            lower_block_constructor_patterns(else_branch, constructors);
        }
        ExprKind::IfLet {
            pattern: selected,
            value,
            then_branch,
            else_branch,
        } => {
            lower_pattern(selected, constructors);
            lower_constructor_patterns(value, constructors);
            lower_block_constructor_patterns(then_branch, constructors);
            lower_block_constructor_patterns(else_branch, constructors);
        }
        ExprKind::LetElse {
            pattern: selected,
            value,
            else_branch,
            body,
        } => {
            lower_pattern(selected, constructors);
            lower_constructor_patterns(value, constructors);
            lower_block_constructor_patterns(else_branch, constructors);
            lower_block_constructor_patterns(body, constructors);
        }
        ExprKind::Match { value, arms } => {
            lower_constructor_patterns(value, constructors);
            for arm in arms {
                lower_pattern(&mut arm.value.pattern, constructors);
                if let Some(guard) = &mut arm.value.guard {
                    lower_constructor_patterns(guard, constructors);
                }
                lower_constructor_patterns(&mut arm.value.value, constructors);
            }
        }
        ExprKind::Int(_)
        | ExprKind::Float(_)
        | ExprKind::String(_)
        | ExprKind::Bytes(_)
        | ExprKind::Atom(_)
        | ExprKind::Variable(_) => {}
    }
}
