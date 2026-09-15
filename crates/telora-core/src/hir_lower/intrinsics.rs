use super::*;
use crate::syntax::kinds::{BindingKind, BlameAction};

impl Lower<'_> {
    pub(super) fn intrinsic(&self, node: NodeRef) -> Result<Shape, ()> {
        let arguments = self.expressions(node);
        if self.rule(node) == Some(Rule::InterpreterIntrinsic) {
            if arguments.len() != 1 {
                return Err(self.error(
                    node,
                    format!(
                        "interpreter! expects exactly one argument, found {}",
                        arguments.len()
                    ),
                ));
            }
            // The type pass already supplies the adapter plan. The old AST's
            // name-based speculative closure expansion was discarded by HIR.
            return Ok(Shape::Node(
                HirKind::Interpreter,
                vec![Input::expr(Role::Operand, arguments[0])],
            ));
        }
        let name = self.required_token(node, Token::Identifier)?;
        self.named_intrinsic(node, name, &arguments)
    }

    pub(super) fn postfix_intrinsic(
        &self,
        node: NodeRef,
        receiver: NodeRef,
        suffix: NodeRef,
    ) -> Result<Shape, ()> {
        let name = self.required_token(suffix, Token::Identifier)?;
        let args = self.child(suffix, Rule::Arguments)?;
        let mut arguments = vec![receiver];
        arguments.extend(self.expressions(args));
        self.named_intrinsic(node, name, &arguments)
    }

    fn named_intrinsic(
        &self,
        node: NodeRef,
        name_node: NodeRef,
        args: &[NodeRef],
    ) -> Result<Shape, ()> {
        let name = self.text(name_node);
        match name.as_ref() {
            "panic" => {
                if args.len() != 1 {
                    return Err(self.error(
                        node,
                        format!("panic! expects exactly one argument, found {}", args.len()),
                    ));
                }
                Ok(Shape::Node(
                    HirKind::Panic,
                    vec![Input::expr(Role::Value, args[0])],
                ))
            }
            "ty" => {
                let [value, target] = args else {
                    return Err(self.error(
                        node,
                        format!(
                            "ty! expects a value and a Type, found {} arguments",
                            args.len()
                        ),
                    ));
                };
                Ok(Shape::Node(
                    HirKind::TypeAscription,
                    vec![
                        Input::expr(Role::Value, *value),
                        Input::with(Role::Target, *target, Mode::Type),
                    ],
                ))
            }
            "dbg" => {
                if !(1..=2).contains(&args.len()) {
                    return Err(self.error(node, format!("dbg! expects an expression and an optional String literal, found {} arguments", args.len())));
                }
                let message = if let Some(message) = args.get(1) {
                    let mut head = *message;
                    while matches!(
                        self.rule(head),
                        Some(Rule::Expression | Rule::Primary | Rule::Braced | Rule::ParenExpr)
                    ) {
                        if self.token(head, Token::Comma).is_some() {
                            break;
                        }
                        let [inner] = self.operands(head)?;
                        head = inner;
                    }
                    if self.rule(head) != Some(Rule::StringExpr)
                        || self.child(head, Rule::StringLiteral).is_err()
                    {
                        return Err(self.error(*message, "dbg! message must be a String literal"));
                    }
                    Some(self.plain_string(self.child(head, Rule::StringLiteral)?)?)
                } else {
                    None
                };
                Ok(Shape::Node(
                    HirKind::Debug {
                        message,
                        expression: self.text(args[0]).into_owned(),
                    },
                    vec![Input::expr(Role::Value, args[0])],
                ))
            }
            "fail" | "blame" | "raise" | "warn" => {
                if args.is_empty() {
                    return Err(self.error(
                        node,
                        format!("{name}! expects a message followed by zero or more subjects"),
                    ));
                }
                let action = match name.as_ref() {
                    "blame" => BlameAction::Build,
                    "raise" => BlameAction::Raise,
                    "warn" => BlameAction::Warn,
                    _ => BlameAction::Fail,
                };
                if matches!(action, BlameAction::Raise | BlameAction::Warn) && args.len() != 1 {
                    return Err(self.error(
                        node,
                        format!("{name}! expects exactly one String or BlameError"),
                    ));
                }
                let mut inputs = vec![Input::expr(Role::Value, args[0])];
                inputs.extend(
                    args[1..]
                        .iter()
                        .map(|node| Input::expr(Role::Subject, *node)),
                );
                Ok(Shape::Node(HirKind::Raise(action), inputs))
            }
            "unwrap" | "ok_or_warn" => {
                if args.len() != 1 {
                    return Err(self.error(
                        node,
                        format!(
                            "{name}! expects exactly one Result value, found {} arguments",
                            args.len()
                        ),
                    ));
                }
                Ok(self.unwrap(node, &name, args[0]))
            }
            "file" | "line" => Err(self.error(
                name_node,
                format!("{name}! is reserved but not implemented"),
            )),
            _ => Err(self.error(name_node, format!("unknown contextual intrinsic {name}!"))),
        }
    }

    fn unwrap(&self, node: NodeRef, name: &str, operand: NodeRef) -> Shape {
        let prefix = format!("${name}:{}", self.location(node).start);
        let variable = |role, suffix| {
            self.synthetic(
                role,
                node,
                HirKind::Variable(format!("{prefix}:{suffix}")),
                vec![],
            )
        };
        let name_node = self.synthetic(
            Role::Name,
            node,
            HirKind::Name(format!("{prefix}:result")),
            vec![],
        );
        let binding = self.synthetic(
            Role::Binding,
            node,
            HirKind::Binding {
                kind: BindingKind::Let,
                initializer: None,
                imported: None,
            },
            vec![name_node, Input::expr(Role::Value, operand)],
        );
        let mut arms = vec![variable(Role::Value, "result")];
        for (tag, suffix) in [("Ok", "payload"), ("Err", "message")] {
            let payload = self.synthetic(
                Role::Pattern,
                node,
                HirKind::PatternName(format!("{prefix}:{suffix}")),
                vec![],
            );
            let callee = self.synthetic(Role::Callee, node, HirKind::Variable(tag.into()), vec![]);
            let pattern = self.synthetic(
                Role::Pattern,
                node,
                HirKind::ConstructorPattern,
                vec![payload, callee],
            );
            let value = if tag == "Err" {
                self.synthetic(
                    Role::Value,
                    node,
                    HirKind::Raise(if name == "ok_or_warn" {
                        BlameAction::Warn
                    } else {
                        BlameAction::Raise
                    }),
                    vec![variable(Role::Value, suffix)],
                )
            } else if name == "ok_or_warn" {
                let callee =
                    self.synthetic(Role::Callee, node, HirKind::Variable("Some".into()), vec![]);
                self.synthetic(
                    Role::Value,
                    node,
                    HirKind::Call,
                    vec![callee, variable(Role::Argument, suffix)],
                )
            } else {
                variable(Role::Value, suffix)
            };
            arms.push(self.synthetic(
                Role::Arm,
                node,
                HirKind::MatchArm { irrefutable: false },
                vec![pattern, value],
            ));
        }
        let matched = self.synthetic(Role::Result, node, HirKind::Match, arms);
        Shape::Desugared(HirKind::Block, vec![binding, matched])
    }
}
