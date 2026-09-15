use super::*;
use crate::syntax::kinds::BindingKind;

enum Entry {
    Bindings(Vec<Input>),
    Destructure {
        syntax: NodeRef,
        pattern: NodeRef,
        value: NodeRef,
        otherwise: Option<NodeRef>,
    },
}

impl Lower<'_> {
    pub(super) fn closure(&self, node: NodeRef) -> Result<Shape, ()> {
        let parameters = self.child(node, Rule::Parameters)?;
        let block = self.child(node, Rule::Block)?;
        let mut inputs = self
            .cst
            .children(parameters)
            .filter(|child| self.rule(*child) == Some(Rule::Parameter))
            .map(|node| Input::with(Role::Parameter, node, Mode::Parameter))
            .collect::<Vec<_>>();
        inputs.push(Input::with(Role::ReturnType, node, Mode::ReturnSlot));
        inputs.push(Input::with(Role::Body, block, Mode::Body));
        Ok(Shape::Node(HirKind::Closure, inputs))
    }

    pub(super) fn parameter(&self, node: NodeRef) -> Result<Shape, ()> {
        let name = self.token(node, Token::Identifier).ok_or(())?;
        let mut inputs = vec![Input::with(Role::Name, name, Mode::Name)];
        if self.token(node, Token::Colon).is_some() {
            inputs.push(match self.expressions(node).into_iter().next() {
                Some(annotation) => Input::with(Role::Annotation, annotation, Mode::Type),
                None => self.synthetic(Role::Annotation, node, HirKind::Missing, vec![]),
            });
        }
        Ok(Shape::Node(HirKind::Parameter, inputs))
    }

    pub(super) fn return_slot(&self, node: NodeRef) -> Result<Shape, ()> {
        let mut inputs = vec![];
        if self.token(node, Token::Arrow).is_some() {
            let block = self.child(node, Rule::Block)?;
            let annotation = self
                .expressions(node)
                .into_iter()
                .find(|child| *child != block)
                .ok_or(())?;
            inputs.push(Input::with(Role::Annotation, annotation, Mode::Type));
        }
        Ok(Shape::Desugared(HirKind::ReturnType, inputs))
    }

    pub(super) fn body(&self, node: NodeRef) -> Result<Shape, ()> {
        let body = if self.rule(node) == Some(Rule::Block) {
            self.child(node, Rule::Body)?
        } else {
            node
        };
        let mut entries = vec![];
        let mut next = Some(body);
        while let Some(body) = next.take() {
            for child in self.cst.children(body) {
                if self.rule(child) == Some(Rule::Body) {
                    next = Some(child);
                } else if !matches!(
                    self.cst.get(child),
                    Node::Token(Token::Whitespace | Token::Comment, _)
                ) {
                    entries.push(child);
                }
            }
        }
        let mut steps = vec![];
        let mut result = Input::with(Role::Result, node, Mode::Unit);
        for (index, child) in entries.iter().copied().enumerate() {
            if Expr::cast(self.cst, child).is_some() {
                let terminated = entries.get(index + 1).is_some_and(|next| {
                    matches!(self.cst.get(*next), Node::Token(Token::Semicolon, _))
                });
                if terminated {
                    steps.push(Entry::Bindings(vec![Input::with(
                        Role::Binding,
                        child,
                        Mode::Discard,
                    )]));
                } else {
                    result = Input::expr(Role::Result, child);
                }
            } else if self.rule(child).is_some() {
                let mut child = child;
                if self.rule(child) == Some(Rule::Binding) {
                    child = self
                        .cst
                        .children(child)
                        .find(|child| self.rule(*child).is_some())
                        .ok_or(())?;
                }
                if matches!(
                    self.rule(child),
                    Some(Rule::LetPatternBinding | Rule::LetElseBinding)
                ) {
                    if self.token(child, Token::Colon).is_some() {
                        return Err(
                            self.error(child, "let else does not support a binding annotation")
                        );
                    }
                    let equal = self.required_token(child, Token::Equal)?;
                    let pattern = self
                        .cst
                        .children(child)
                        .find(|item| self.is_pattern(*item) && item.0 < equal.0)
                        .ok_or(())?;
                    let value = self
                        .expressions(child)
                        .into_iter()
                        .find(|item| item.0 > equal.0)
                        .ok_or(())?;
                    let otherwise = if self.rule(child) == Some(Rule::LetElseBinding) {
                        Some(self.child(child, Rule::Block)?)
                    } else {
                        None
                    };
                    steps.push(Entry::Destructure {
                        syntax: child,
                        pattern,
                        value,
                        otherwise,
                    });
                } else {
                    steps.push(Entry::Bindings(self.binding_inputs(child, false)?));
                }
            }
        }
        let mut reversed = vec![];
        for step in steps.into_iter().rev() {
            match step {
                Entry::Bindings(bindings) => reversed.extend(bindings.into_iter().rev()),
                Entry::Destructure {
                    syntax,
                    pattern,
                    value,
                    otherwise,
                } => {
                    let mut continuation = reversed.drain(..).rev().collect::<Vec<_>>();
                    continuation.push(result);
                    let pattern = Input::with(Role::Pattern, pattern, Mode::Pattern);
                    let value = Input::expr(Role::Value, value);
                    result = if let Some(otherwise) = otherwise {
                        let body = self.synthetic(Role::Body, node, HirKind::Block, continuation);
                        self.synthetic(
                            Role::Result,
                            syntax,
                            HirKind::LetElse,
                            vec![
                                pattern,
                                value,
                                Input::with(Role::Else, otherwise, Mode::Body),
                                body,
                            ],
                        )
                    } else {
                        let body = self.synthetic(Role::Value, node, HirKind::Block, continuation);
                        let arm = self.synthetic(
                            Role::Arm,
                            syntax,
                            HirKind::MatchArm { irrefutable: true },
                            vec![pattern, body],
                        );
                        self.synthetic(Role::Result, syntax, HirKind::Match, vec![value, arm])
                    };
                }
            }
        }
        let mut inputs = reversed.into_iter().rev().collect::<Vec<_>>();
        inputs.push(result);
        Ok(Shape::Node(HirKind::Block, inputs))
    }

    pub(super) fn binding(&self, node: NodeRef) -> Result<Shape, ()> {
        if self.rule(node) == Some(Rule::Error) {
            // Parsing already diagnosed this unavailable declaration.
            return Ok(Shape::Node(HirKind::Missing, vec![]));
        }
        if self.rule(node) == Some(Rule::Binding) {
            let inner = self
                .cst
                .children(node)
                .find(|child| self.rule(*child).is_some())
                .ok_or(())?;
            return Ok(Shape::Alias(inner, Mode::Binding));
        }
        if self.rule(node) != Some(Rule::LetBinding) {
            return self.declaration(node);
        }
        let name = self.token(node, Token::Identifier).ok_or(())?;
        let mut inputs = vec![Input::with(Role::Name, name, Mode::Name)];
        let mut expressions = self.expressions(node).into_iter();
        if self.token(node, Token::Colon).is_some() {
            let annotation = expressions.next().ok_or(())?;
            inputs.push(Input::with(Role::Annotation, annotation, Mode::Type));
        }
        let value = expressions.next();
        if expressions.next().is_some() {
            return Err(self.error(node, "unexpected binding expression"));
        }
        inputs.push(self.optional_expression(Role::Value, node, value));
        Ok(Shape::Node(
            HirKind::Binding {
                kind: BindingKind::Let,
                initializer: None,
                imported: None,
            },
            inputs,
        ))
    }

    pub(super) fn discard(&self, node: NodeRef) -> Result<Shape, ()> {
        // A generated name is a separate semantic node with the statement as
        // its source. Its identity must not be shared with the expression.
        Ok(Shape::Desugared(
            HirKind::Binding {
                kind: BindingKind::Let,
                initializer: None,
                imported: None,
            },
            vec![
                Input::with(Role::Name, node, Mode::DiscardName),
                Input::expr(Role::Value, node),
            ],
        ))
    }
}
