use super::*;
use crate::syntax::kinds::BindingKind;

impl Lower<'_> {
    pub(super) fn closure(&self, node: NodeRef) -> Result<Shape, Diagnostic> {
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

    pub(super) fn parameter(&self, node: NodeRef) -> Result<Shape, Diagnostic> {
        let name = self
            .token(node, Token::Identifier)
            .ok_or_else(|| self.error(node, "parameter has no name"))?;
        let mut inputs = vec![Input::with(Role::Name, name, Mode::Name)];
        if self.token(node, Token::Colon).is_some() {
            let [annotation] = self.operands(node)?;
            inputs.push(Input::with(Role::Annotation, annotation, Mode::Type));
        }
        Ok(Shape::Node(HirKind::Parameter, inputs))
    }

    pub(super) fn return_slot(&self, node: NodeRef) -> Result<Shape, Diagnostic> {
        let mut inputs = vec![];
        if self.token(node, Token::Arrow).is_some() {
            let block = self.child(node, Rule::Block)?;
            let annotation = self
                .expressions(node)
                .into_iter()
                .find(|child| *child != block)
                .ok_or_else(|| self.error(node, "closure has no return annotation"))?;
            inputs.push(Input::with(Role::Annotation, annotation, Mode::Type));
        }
        Ok(Shape::Desugared(HirKind::ReturnType, inputs))
    }

    pub(super) fn body(&self, node: NodeRef) -> Result<Shape, Diagnostic> {
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
        let mut inputs = vec![];
        let mut has_result = false;
        for (index, child) in entries.iter().copied().enumerate() {
            if Expr::cast(self.cst, child).is_some() {
                let terminated = entries.get(index + 1).is_some_and(|next| {
                    matches!(self.cst.get(*next), Node::Token(Token::Semicolon, _))
                });
                if terminated {
                    inputs.push(Input::with(Role::Binding, child, Mode::Discard));
                } else {
                    inputs.push(Input::expr(Role::Result, child));
                    has_result = true;
                }
            } else if self.rule(child).is_some() {
                inputs.push(Input::with(Role::Binding, child, Mode::Binding));
            }
        }
        if !has_result {
            inputs.push(Input::with(Role::Result, node, Mode::Unit));
        }
        Ok(Shape::Node(HirKind::Block, inputs))
    }

    pub(super) fn binding(&self, node: NodeRef) -> Result<Shape, Diagnostic> {
        if self.rule(node) == Some(Rule::Binding) {
            let inner = self
                .cst
                .children(node)
                .find(|child| self.rule(*child).is_some())
                .ok_or_else(|| self.error(node, "empty binding"))?;
            return Ok(Shape::Alias(inner, Mode::Binding));
        }
        if self.rule(node) != Some(Rule::LetBinding) {
            return Err(self.error(
                node,
                format!(
                    "CST-to-HIR binding is not implemented for {:?}",
                    self.rule(node)
                ),
            ));
        }
        let name = self
            .token(node, Token::Identifier)
            .ok_or_else(|| self.error(node, "binding has no name"))?;
        let mut inputs = vec![Input::with(Role::Name, name, Mode::Name)];
        let mut expressions = self.expressions(node).into_iter();
        if self.token(node, Token::Colon).is_some() {
            let annotation = expressions
                .next()
                .ok_or_else(|| self.error(node, "binding has no annotation"))?;
            inputs.push(Input::with(Role::Annotation, annotation, Mode::Type));
        }
        let value = expressions
            .next()
            .ok_or_else(|| self.error(node, "binding has no value"))?;
        if expressions.next().is_some() {
            return Err(self.error(node, "unexpected binding expression"));
        }
        inputs.push(Input::expr(Role::Value, value));
        Ok(Shape::Node(
            HirKind::Binding {
                kind: BindingKind::Let,
                initializer: None,
                imported: None,
            },
            inputs,
        ))
    }

    pub(super) fn discard(&self, node: NodeRef) -> Result<Shape, Diagnostic> {
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
