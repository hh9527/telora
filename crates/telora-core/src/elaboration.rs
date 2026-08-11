use crate::Location;
use crate::ast::*;
use crate::types::PropagationFamily;
use std::collections::HashMap;

pub(crate) fn elaborate_program(
    program: &mut Program,
    families: &HashMap<Location, PropagationFamily>,
) {
    let mut elaborator = Elaborator { families, next: 0 };
    elaborator.block(&mut program.value.body);
}

struct Elaborator<'a> {
    families: &'a HashMap<Location, PropagationFamily>,
    next: u32,
}

impl Elaborator<'_> {
    fn block(&mut self, block: &mut Block) {
        for binding in &mut block.value.bindings {
            if let Some(annotation) = &mut binding.value.annotation {
                self.expression(annotation);
            }
            self.expression(&mut binding.value.value);
        }
        self.expression(&mut block.value.result);
    }

    fn expression(&mut self, expression: &mut Expr) {
        match &mut expression.value {
            ExprKind::InterpolatedString(parts) => {
                for part in parts {
                    if let StringPartKind::Expression(expression) = &mut part.value {
                        self.expression(expression);
                    }
                }
            }
            ExprKind::Array(items) | ExprKind::Tuple(items) => {
                for item in items {
                    self.expression(item);
                }
            }
            ExprKind::Spread(operand) => self.expression(operand),
            ExprKind::Dict(fields) => {
                for field in fields {
                    self.expression(&mut field.value.value);
                }
            }
            ExprKind::Block(block) => self.block(block),
            ExprKind::Unary { operator, operand } => {
                self.expression(operand);
                if operator.value == UnaryOperator::Not {
                    let condition = operand.clone();
                    let atom =
                        |name: &str| located(ExprKind::Atom(name.into()), expression.location);
                    let block = |result: Expr| {
                        located(
                            BlockKind {
                                bindings: Vec::new(),
                                result: Box::new(result),
                            },
                            expression.location,
                        )
                    };
                    expression.value = ExprKind::If {
                        condition,
                        then_branch: block(atom("False")),
                        else_branch: block(atom("True")),
                    };
                }
            }
            ExprKind::Propagate { operand } => {
                self.expression(operand);
                let family = self.families[&expression.location];
                let operand = (**operand).clone();
                expression.value = self.propagation(operand, family, expression.location);
            }
            ExprKind::Return { value } => self.expression(value),
            ExprKind::Panic { message } => self.expression(message),
            ExprKind::Raise { error } => self.expression(error),
            ExprKind::Binary {
                operator,
                left,
                right,
            } => {
                self.expression(left);
                self.expression(right);
                if matches!(operator.value, BinaryOperator::And | BinaryOperator::Or) {
                    let left = left.clone();
                    let right = right.clone();
                    let atom =
                        |name: &str| located(ExprKind::Atom(name.into()), expression.location);
                    let (then_result, else_result) = match operator.value {
                        BinaryOperator::And => ((*right).clone(), atom("False")),
                        BinaryOperator::Or => (atom("True"), (*right).clone()),
                        _ => unreachable!(),
                    };
                    let block = |result: Expr| {
                        located(
                            BlockKind {
                                bindings: Vec::new(),
                                result: Box::new(result),
                            },
                            expression.location,
                        )
                    };
                    expression.value = ExprKind::If {
                        condition: left,
                        then_branch: block(then_result),
                        else_branch: block(else_result),
                    };
                }
            }
            ExprKind::Field { receiver, .. } => self.expression(receiver),
            ExprKind::Index { receiver, index } => {
                self.expression(receiver);
                self.expression(index);
            }
            ExprKind::TupleProjection { receiver, .. } => self.expression(receiver),
            ExprKind::Call { callee, arguments } => {
                self.expression(callee);
                for argument in arguments {
                    self.expression(argument);
                }
            }
            ExprKind::TypeApply { callee, arguments } => {
                self.expression(callee);
                for argument in arguments {
                    if let TypeArgumentKind::Explicit(argument) = &mut argument.value {
                        self.expression(argument);
                    }
                }
            }
            ExprKind::Interpreter {
                operand,
                elaboration,
            } => {
                self.expression(operand);
                self.expression(elaboration);
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
                    self.expression(annotation);
                }
                self.block(body);
            }
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expression(condition);
                self.block(then_branch);
                self.block(else_branch);
            }
            ExprKind::IfLet {
                pattern,
                value,
                then_branch,
                else_branch,
            } => {
                self.expression(value);
                self.block(then_branch);
                self.block(else_branch);
                expression.value = ExprKind::Match {
                    value: value.clone(),
                    arms: vec![
                        located(
                            MatchArmKind {
                                pattern: pattern.clone(),
                                guard: None,
                                value: located(
                                    ExprKind::Block(then_branch.clone()),
                                    then_branch.location,
                                ),
                                irrefutable_required: false,
                            },
                            expression.location,
                        ),
                        located(
                            MatchArmKind {
                                pattern: located(PatternKind::Wildcard, expression.location),
                                guard: None,
                                value: located(
                                    ExprKind::Block(else_branch.clone()),
                                    else_branch.location,
                                ),
                                irrefutable_required: false,
                            },
                            expression.location,
                        ),
                    ],
                };
            }
            ExprKind::LetElse {
                pattern,
                value,
                else_branch,
                body,
            } => {
                self.expression(value);
                self.block(else_branch);
                self.block(body);
                expression.value = ExprKind::Match {
                    value: value.clone(),
                    arms: vec![
                        located(
                            MatchArmKind {
                                pattern: pattern.clone(),
                                guard: None,
                                value: located(ExprKind::Block(body.clone()), body.location),
                                irrefutable_required: false,
                            },
                            expression.location,
                        ),
                        located(
                            MatchArmKind {
                                pattern: located(PatternKind::Wildcard, expression.location),
                                guard: None,
                                value: located(
                                    ExprKind::Block(else_branch.clone()),
                                    else_branch.location,
                                ),
                                irrefutable_required: false,
                            },
                            expression.location,
                        ),
                    ],
                };
            }
            ExprKind::Match { value, arms } => {
                self.expression(value);
                for arm in arms {
                    if let Some(guard) = &mut arm.value.guard {
                        self.expression(guard);
                    }
                    self.expression(&mut arm.value.value);
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

    fn propagation(
        &mut self,
        operand: Expr,
        family: PropagationFamily,
        location: Location,
    ) -> ExprKind {
        let index = self.next;
        self.next += 1;
        let subject = format!("$propagate:{index}:subject");
        let payload = format!("$propagate:{index}:payload");
        let identifier = |name: &str| located(name.to_owned(), location);
        let variable = |name: &str| located(ExprKind::Variable(identifier(name)), location);
        let success_tag = match family {
            PropagationFamily::Option => "Some",
            PropagationFamily::Result => "Ok",
        };
        let failure_tag = match family {
            PropagationFamily::Option => "None",
            PropagationFamily::Result => "Err",
        };
        let success_pattern = located(
            PatternKind::Tagged {
                tag: success_tag.into(),
                payload: Box::new(located(
                    PatternKind::Binding(identifier(&payload)),
                    location,
                )),
            },
            location,
        );
        let failure_pattern = match family {
            PropagationFamily::Option => located(PatternKind::Atom(failure_tag.into()), location),
            PropagationFamily::Result => located(
                PatternKind::Tagged {
                    tag: failure_tag.into(),
                    payload: Box::new(located(PatternKind::Wildcard, location)),
                },
                location,
            ),
        };
        let arms = vec![
            located(
                MatchArmKind {
                    pattern: success_pattern,
                    guard: None,
                    value: variable(&payload),
                    irrefutable_required: false,
                },
                location,
            ),
            located(
                MatchArmKind {
                    pattern: failure_pattern,
                    guard: None,
                    value: located(
                        ExprKind::Return {
                            value: Box::new(variable(&subject)),
                        },
                        location,
                    ),
                    irrefutable_required: false,
                },
                location,
            ),
        ];
        ExprKind::Block(located(
            BlockKind {
                bindings: vec![located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::Let,
                        imported_name: None,
                        name: identifier(&subject),
                        type_parameters: Vec::new(),
                        annotation: None,
                        value: operand,
                    },
                    location,
                )],
                result: Box::new(located(
                    ExprKind::Match {
                        value: Box::new(variable(&subject)),
                        arms,
                    },
                    location,
                )),
            },
            location,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{SourceDatabase, TextRange};

    fn location() -> Location {
        let mut sources = SourceDatabase::default();
        let source = sources.add("test", "input?");
        Location::new(source, TextRange::new(0, 6).unwrap())
    }

    #[test]
    fn propagation_elaborates_to_hygienic_core_forms() {
        let location = location();
        let operand = located(
            ExprKind::Variable(located("input".into(), location)),
            location,
        );
        let mut elaborator = Elaborator {
            families: &HashMap::new(),
            next: 0,
        };
        let ExprKind::Block(block) =
            elaborator.propagation(operand, PropagationFamily::Result, location)
        else {
            panic!("propagation must elaborate to a block")
        };
        let subject = &block.value.bindings[0].value.name.value;
        assert!(subject.starts_with("$propagate:"));
        let ExprKind::Match { arms, .. } = &block.value.result.value else {
            panic!("elaborated block must end in match")
        };
        assert!(
            matches!(arms[0].value.pattern.value, PatternKind::Tagged { ref tag, .. } if tag == "Ok")
        );
        assert!(matches!(arms[1].value.value.value, ExprKind::Return { .. }));
        assert_eq!(arms[1].value.value.location, location);
    }
}
