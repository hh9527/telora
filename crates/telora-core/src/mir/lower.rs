//! Syntax-only lowering. Every expression is moved into the session arena;
//! references and type positions receive slots without consulting a resolver.
use super::*;
use crate::ast::{self, ExprKind as E, PatternKind as P};

pub(crate) struct Lower<'a> {
    pub mir: &'a mut Mir,
    pub module: ModuleId,
}

impl Lower<'_> {
    fn type_operation(name: &str) -> Option<TypeOperation> {
        match name {
            "\0telora_function_type" => Some(TypeOperation::Function),
            "\0telora_tuple_type" => Some(TypeOperation::Tuple),
            "\0telora_unit_type" => Some(TypeOperation::Unit),
            "\0telora_struct" => Some(TypeOperation::Struct),
            "\0telora_newtype" => Some(TypeOperation::Newtype),
            "\0telora_enum" => Some(TypeOperation::Enum),
            _ => None,
        }
    }
    fn node(&mut self, location: Location, kind: HirKind, children: Vec<Edge>) -> HirId {
        self.mir.node(self.module, location, kind, children)
    }
    fn name(&mut self, name: ast::Identifier) -> HirId {
        self.node(name.location, HirKind::Name(name.value), vec![])
    }
    fn expression_edge(&mut self, children: &mut Vec<Edge>, role: Role, expr: ast::Expr) {
        children.push(Edge {
            role,
            node: self.expr(expr),
        });
    }
    pub fn body(
        &mut self,
        location: Location,
        bindings: Vec<ast::Binding>,
        result: Option<ast::Expr>,
    ) -> HirId {
        let mut children = bindings
            .into_iter()
            .map(|binding| Edge {
                role: Role::Binding,
                node: self.binding(binding),
            })
            .collect::<Vec<_>>();
        if let Some(result) = result {
            self.expression_edge(&mut children, Role::Result, result);
        }
        self.node(location, HirKind::Block, children)
    }
    fn block(&mut self, block: ast::Block) -> HirId {
        self.body(
            block.location,
            block.value.bindings,
            Some(*block.value.result),
        )
    }
    fn decorators(&mut self, children: &mut Vec<Edge>, decorators: Vec<ast::Decorator>) {
        for decorator in decorators {
            let mut edges = vec![];
            self.expression_edge(&mut edges, Role::Callee, decorator.value.callee);
            for argument in decorator.value.arguments {
                self.expression_edge(&mut edges, Role::Argument, argument);
            }
            let node = self.node(
                decorator.location,
                HirKind::Decorator {
                    configured: decorator.value.configured,
                },
                edges,
            );
            children.push(Edge {
                role: Role::Decorator,
                node,
            });
        }
    }
    fn binding(&mut self, binding: ast::Binding) -> HirId {
        let b = binding.value;
        let mut edges = vec![Edge {
            role: Role::Name,
            node: self.name(b.name),
        }];
        let mut bounds = b.type_parameter_bounds.into_iter();
        for name in b.type_parameters {
            let location = name.location;
            let mut parameter = vec![Edge {
                role: Role::Name,
                node: self.name(name),
            }];
            for bound in bounds.next().unwrap_or_default() {
                self.expression_edge(&mut parameter, Role::Bound, bound);
            }
            let node = self.node(location, HirKind::TypeParameter, parameter);
            edges.push(Edge {
                role: Role::TypeParameter,
                node,
            });
        }
        self.decorators(&mut edges, b.decorators);
        if let Some(annotation) = b.annotation {
            self.expression_edge(&mut edges, Role::Annotation, annotation);
        }
        if b.kind == ast::BindingKind::NativeType {
            let E::Int(slot) = b.value.value else {
                unreachable!("native type slot")
            };
            let node = self.node(b.value.location, HirKind::NativeTypeSlot(slot), vec![]);
            edges.push(Edge {
                role: Role::Value,
                node,
            });
        } else {
            self.expression_edge(&mut edges, Role::Value, b.value);
        }
        self.node(
            binding.location,
            HirKind::Binding {
                kind: b.kind,
                initializer: b.declared_initializer,
                imported: b.imported_name.map(|name| name.value),
            },
            edges,
        )
    }
    fn pattern(&mut self, pattern: ast::Pattern) -> HirId {
        let mut edges = vec![];
        let kind = match pattern.value {
            P::Wildcard => HirKind::Wildcard,
            P::Binding(name) => HirKind::PatternName(name.value),
            P::Int(value) => HirKind::Int(value),
            P::Float(value) => HirKind::Float(value),
            P::String(value) => HirKind::String(value),
            P::Atom(value) => {
                let callee = self.node(pattern.location, HirKind::Variable(value), vec![]);
                edges.push(Edge {
                    role: Role::Callee,
                    node: callee,
                });
                HirKind::ConstructorPattern
            }
            P::Tagged { tag, payload } => {
                edges.push(Edge {
                    role: Role::Pattern,
                    node: self.pattern(*payload),
                });
                let callee = self.node(pattern.location, HirKind::Variable(tag), vec![]);
                edges.push(Edge {
                    role: Role::Callee,
                    node: callee,
                });
                HirKind::ConstructorPattern
            }
            P::Constructor {
                constructor,
                payload,
            } => {
                self.expression_edge(&mut edges, Role::Callee, *constructor);
                if let Some(payload) = payload {
                    edges.push(Edge {
                        role: Role::Pattern,
                        node: self.pattern(*payload),
                    });
                }
                HirKind::ConstructorPattern
            }
            P::Tuple(items) => {
                for item in items {
                    edges.push(Edge {
                        role: Role::Item,
                        node: self.pattern(item),
                    });
                }
                HirKind::TuplePattern
            }
            P::Struct(fields) => {
                for field in fields {
                    let location = field.name.location;
                    let children = vec![
                        Edge {
                            role: Role::Name,
                            node: self.name(field.name),
                        },
                        Edge {
                            role: Role::Pattern,
                            node: self.pattern(field.pattern),
                        },
                    ];
                    let node = self.node(location, HirKind::PatternField, children);
                    edges.push(Edge {
                        role: Role::Field,
                        node,
                    });
                }
                HirKind::StructPattern
            }
        };
        self.node(pattern.location, kind, edges)
    }
    fn expr(&mut self, expression: ast::Expr) -> HirId {
        // Parser-generated type syntax is an IR operation, not a source symbol
        // or a call which could be dispatched to a VM.
        if let E::Call { callee, .. } = &expression.value
            && let E::Variable(name) = &callee.value
            && let Some(operation) = Self::type_operation(&name.value)
        {
            let E::Call { arguments, .. } = expression.value else {
                unreachable!()
            };
            let mut edges = vec![];
            if matches!(
                operation,
                TypeOperation::Struct | TypeOperation::Newtype | TypeOperation::Enum
            ) {
                // The parser encodes declarations as helper(context, members).
                // Retain source members directly; the synthetic context is not a value.
                let members = arguments.into_iter().last().expect("type members");
                let E::Dict(fields) = members.value else {
                    unreachable!("type member table")
                };
                for field in fields {
                    let nullary = operation == TypeOperation::Enum
                        && matches!(&field.value.value.value, E::Atom(name) if name == "None");
                    let mut children = vec![];
                    self.decorators(&mut children, field.value.decorators);
                    if !nullary {
                        self.expression_edge(&mut children, Role::Annotation, field.value.value);
                    }
                    let name = field.value.name.expect("named type member").value;
                    let node = self.node(
                        field.location,
                        HirKind::TypeMember { name, nullary },
                        children,
                    );
                    edges.push(Edge {
                        role: Role::Field,
                        node,
                    });
                }
                return self.node(
                    expression.location,
                    HirKind::TypeOperation(operation),
                    edges,
                );
            }
            for (index, argument) in arguments.into_iter().enumerate() {
                if index == 0
                    && matches!(operation, TypeOperation::Function | TypeOperation::Tuple)
                    && let E::Array(items) = argument.value
                {
                    for item in items {
                        self.expression_edge(&mut edges, Role::Argument, item);
                    }
                } else {
                    self.expression_edge(&mut edges, Role::Argument, argument);
                }
            }
            return self.node(
                expression.location,
                HirKind::TypeOperation(operation),
                edges,
            );
        }
        if let E::Variable(name) = &expression.value
            && let Some(operation) = Self::type_operation(&name.value)
        {
            return self.node(
                expression.location,
                HirKind::TypeOperation(operation),
                vec![],
            );
        }
        let mut edges = vec![];
        let kind = match expression.value {
            E::Int(v) => HirKind::Int(v),
            E::Float(v) => HirKind::Float(v),
            E::String(v) => HirKind::String(v),
            E::Bytes(v) => HirKind::Bytes(v),
            E::Atom(v) => HirKind::Variable(v),
            E::Variable(v) => HirKind::Variable(v.value),
            E::Array(items) => {
                for item in items {
                    self.expression_edge(&mut edges, Role::Item, item);
                }
                HirKind::Array
            }
            E::Tuple(items) => {
                for item in items {
                    self.expression_edge(&mut edges, Role::Item, item);
                }
                HirKind::Tuple
            }
            E::InterpolatedString(parts) => {
                for part in parts {
                    let node = match part.value {
                        ast::StringPartKind::Text(text) => {
                            self.node(part.location, HirKind::String(text), vec![])
                        }
                        ast::StringPartKind::Expression(expr) => self.expr(expr),
                    };
                    edges.push(Edge {
                        role: Role::Part,
                        node,
                    });
                }
                HirKind::InterpolatedString
            }
            E::Dict(fields) => {
                for field in fields {
                    let mut children = vec![];
                    if let Some(name) = field.value.name {
                        children.push(Edge {
                            role: Role::Name,
                            node: self.name(name),
                        });
                    }
                    self.decorators(&mut children, field.value.decorators);
                    self.expression_edge(&mut children, Role::Value, field.value.value);
                    let node = self.node(field.location, HirKind::DictField, children);
                    edges.push(Edge {
                        role: Role::Field,
                        node,
                    });
                }
                HirKind::Dict
            }
            E::Block(block) => return self.block(block),
            E::Spread(v) => {
                self.expression_edge(&mut edges, Role::Operand, *v);
                HirKind::Spread
            }
            E::TypeSyntax(v) => {
                self.expression_edge(&mut edges, Role::Operand, *v);
                HirKind::TypeSyntax
            }
            E::TypeMetadata(v) => {
                self.expression_edge(&mut edges, Role::Operand, *v);
                HirKind::TypeMetadata
            }
            E::Unary { operator, operand } => {
                self.expression_edge(&mut edges, Role::Operand, *operand);
                HirKind::Unary(operator.value)
            }
            E::Propagate { operand } => {
                self.expression_edge(&mut edges, Role::Operand, *operand);
                HirKind::Propagate
            }
            E::Return { value } => {
                self.expression_edge(&mut edges, Role::Value, *value);
                HirKind::Return
            }
            E::Panic { message } => {
                self.expression_edge(&mut edges, Role::Value, *message);
                HirKind::Panic
            }
            E::Raise {
                action,
                message,
                subjects,
            } => {
                self.expression_edge(&mut edges, Role::Value, *message);
                for subject in subjects {
                    self.expression_edge(&mut edges, Role::Subject, subject);
                }
                HirKind::Raise(action)
            }
            E::Debug {
                value,
                message,
                expression,
            } => {
                self.expression_edge(&mut edges, Role::Value, *value);
                HirKind::Debug {
                    message,
                    expression,
                }
            }
            E::TypeAscription { value, target } => {
                self.expression_edge(&mut edges, Role::Value, *value);
                self.expression_edge(&mut edges, Role::Target, *target);
                HirKind::TypeAscription
            }
            E::CheckedCast { value, target } => {
                self.expression_edge(&mut edges, Role::Value, *value);
                self.expression_edge(&mut edges, Role::Target, *target);
                HirKind::CheckedCast
            }
            E::DynProject {
                namespace,
                target,
                value,
            } => {
                self.expression_edge(&mut edges, Role::Namespace, *namespace);
                self.expression_edge(&mut edges, Role::Target, *target);
                self.expression_edge(&mut edges, Role::Value, *value);
                HirKind::DynProject
            }
            E::Binary {
                operator,
                left,
                right,
            } => {
                self.expression_edge(&mut edges, Role::Left, *left);
                self.expression_edge(&mut edges, Role::Right, *right);
                HirKind::Binary(operator.value)
            }
            E::Field { receiver, field } => {
                self.expression_edge(&mut edges, Role::Receiver, *receiver);
                edges.push(Edge {
                    role: Role::Name,
                    node: self.name(field),
                });
                HirKind::Field
            }
            E::FieldProjection { receiver, fields } => {
                self.expression_edge(&mut edges, Role::Receiver, *receiver);
                for (from, to) in fields {
                    edges.push(Edge {
                        role: Role::Name,
                        node: self.name(from),
                    });
                    edges.push(Edge {
                        role: Role::Target,
                        node: self.name(to),
                    });
                }
                HirKind::FieldProjection
            }
            E::Index { receiver, index } => {
                self.expression_edge(&mut edges, Role::Receiver, *receiver);
                self.expression_edge(&mut edges, Role::Index, *index);
                HirKind::Index
            }
            E::TupleProjection { receiver, index } => {
                self.expression_edge(&mut edges, Role::Receiver, *receiver);
                HirKind::TupleProjection(index.value)
            }
            E::Call { callee, arguments } => {
                self.expression_edge(&mut edges, Role::Callee, *callee);
                for arg in arguments {
                    self.expression_edge(&mut edges, Role::Argument, arg);
                }
                HirKind::Call
            }
            E::TypeApply { callee, arguments } => {
                self.expression_edge(&mut edges, Role::Callee, *callee);
                for arg in arguments {
                    let node = match arg.value {
                        ast::TypeArgumentKind::Explicit(expr) => self.expr(expr),
                        ast::TypeArgumentKind::Infer => {
                            self.node(arg.location, HirKind::InferredTypeArgument, vec![])
                        }
                    };
                    edges.push(Edge {
                        role: Role::Argument,
                        node,
                    });
                }
                HirKind::TypeApply
            }
            E::Interpreter {
                operand,
                elaboration,
            } => {
                self.expression_edge(&mut edges, Role::Operand, *operand);
                self.expression_edge(&mut edges, Role::Elaboration, *elaboration);
                HirKind::Interpreter
            }
            E::Closure {
                parameters,
                result_annotation,
                body,
            } => {
                for parameter in parameters {
                    let location = parameter.name.location;
                    let mut children = vec![Edge {
                        role: Role::Name,
                        node: self.name(parameter.name),
                    }];
                    if let Some(annotation) = parameter.annotation {
                        self.expression_edge(&mut children, Role::Annotation, annotation);
                    }
                    let node = self.node(location, HirKind::Parameter, children);
                    edges.push(Edge {
                        role: Role::Parameter,
                        node,
                    });
                }
                let mut result = vec![];
                if let Some(annotation) = result_annotation {
                    self.expression_edge(&mut result, Role::Annotation, *annotation);
                }
                let node = self.node(body.location, HirKind::ReturnType, result);
                edges.push(Edge {
                    role: Role::ReturnType,
                    node,
                });
                edges.push(Edge {
                    role: Role::Body,
                    node: self.block(body),
                });
                HirKind::Closure
            }
            E::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.expression_edge(&mut edges, Role::Condition, *condition);
                edges.push(Edge {
                    role: Role::Then,
                    node: self.block(then_branch),
                });
                edges.push(Edge {
                    role: Role::Else,
                    node: self.block(else_branch),
                });
                HirKind::If
            }
            E::IfLet {
                pattern,
                value,
                then_branch,
                else_branch,
            } => {
                edges.push(Edge {
                    role: Role::Pattern,
                    node: self.pattern(pattern),
                });
                self.expression_edge(&mut edges, Role::Value, *value);
                edges.push(Edge {
                    role: Role::Then,
                    node: self.block(then_branch),
                });
                edges.push(Edge {
                    role: Role::Else,
                    node: self.block(else_branch),
                });
                HirKind::IfLet
            }
            E::LetElse {
                pattern,
                value,
                else_branch,
                body,
            } => {
                edges.push(Edge {
                    role: Role::Pattern,
                    node: self.pattern(pattern),
                });
                self.expression_edge(&mut edges, Role::Value, *value);
                edges.push(Edge {
                    role: Role::Else,
                    node: self.block(else_branch),
                });
                edges.push(Edge {
                    role: Role::Body,
                    node: self.block(body),
                });
                HirKind::LetElse
            }
            E::Match { value, arms } => {
                self.expression_edge(&mut edges, Role::Value, *value);
                for arm in arms {
                    let mut children = vec![Edge {
                        role: Role::Pattern,
                        node: self.pattern(arm.value.pattern),
                    }];
                    if let Some(guard) = arm.value.guard {
                        self.expression_edge(&mut children, Role::Guard, guard);
                    }
                    self.expression_edge(&mut children, Role::Value, arm.value.value);
                    let node = self.node(
                        arm.location,
                        HirKind::MatchArm {
                            irrefutable: arm.value.irrefutable_required,
                        },
                        children,
                    );
                    edges.push(Edge {
                        role: Role::Arm,
                        node,
                    });
                }
                HirKind::Match
            }
        };
        self.node(expression.location, kind, edges)
    }
}
