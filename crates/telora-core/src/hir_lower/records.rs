use super::*;

impl Lower<'_> {
    pub(super) fn record(&self, node: NodeRef) -> Result<Shape, ()> {
        Ok(Shape::Node(
            HirKind::Dict,
            self.cst
                .children(node)
                .filter(|child| {
                    matches!(self.rule(*child), Some(Rule::DictField | Rule::SpreadExpr))
                })
                .map(|node| Input::with(Role::Field, node, Mode::DictField))
                .collect(),
        ))
    }

    pub(super) fn dict_field(&self, node: NodeRef) -> Result<Shape, ()> {
        if self.rule(node) == Some(Rule::SpreadExpr) {
            return Ok(Shape::Node(
                HirKind::DictField,
                vec![Input::expr(Role::Value, node)],
            ));
        }
        let key = self
            .cst
            .children(node)
            .find(|child| {
                matches!(self.cst.get(*child), Node::Token(Token::Identifier, _))
                    || self.rule(*child) == Some(Rule::StringLiteral)
            })
            .ok_or(())?;
        let name = if self.rule(key) == Some(Rule::StringLiteral) {
            self.plain_string(key)?
        } else {
            self.text(key).into_owned()
        };
        if self
            .cst
            .children(node)
            .any(|child| self.rule(child) == Some(Rule::Decorator))
        {
            return Err(self.error(node,
                "decorators are only supported on concrete nominal type declarations; Dict fields do not have property identity"));
        }
        let mut inputs = vec![self.synthetic(Role::Name, key, HirKind::Name(name.clone()), vec![])];
        if let Some(colon) = self.token(node, Token::Colon) {
            let value = self
                .expressions(node)
                .into_iter()
                .find(|child| child.0 > colon.0);
            inputs.push(self.optional_expression(Role::Value, node, value));
        } else {
            inputs.push(self.synthetic(Role::Value, key, HirKind::Variable(name), vec![]));
        }
        Ok(Shape::Node(HirKind::DictField, inputs))
    }

    pub(super) fn decorator(&self, node: NodeRef) -> Result<Shape, ()> {
        let path = self.child(node, Rule::DecoratorPath)?;
        let names = self
            .cst
            .children(path)
            .filter(|child| matches!(self.cst.get(*child), Node::Token(Token::Identifier, _)))
            .collect::<Vec<_>>();
        let last = *names.last().ok_or(())?;
        let check = names.len() == 1 && self.text(last) == "check";
        let mut inputs = vec![];
        if !check {
            inputs.push(Input::with(Role::Callee, path, Mode::Path(last)));
        }
        let arguments = self.child(node, Rule::Arguments).ok();
        if let Some(arguments) = arguments {
            inputs.extend(
                self.expressions(arguments)
                    .into_iter()
                    .map(|node| Input::expr(Role::Argument, node)),
            );
        }
        Ok(Shape::Node(
            if check {
                HirKind::ConstructionCheck {
                    configured: arguments.is_some(),
                }
            } else {
                HirKind::Decorator {
                    configured: arguments.is_some(),
                }
            },
            inputs,
        ))
    }

    pub(super) fn decorators(&self, node: NodeRef) -> Vec<Input> {
        self.cst
            .children(node)
            .filter(|child| self.rule(*child) == Some(Rule::Decorator))
            .map(|node| Input::with(Role::Decorator, node, Mode::Decorator))
            .collect()
    }

    pub(super) fn field_projection(
        &self,
        node: NodeRef,
        receiver: NodeRef,
        suffix: NodeRef,
    ) -> Result<Shape, ()> {
        let mut inputs = vec![Input::expr(Role::Receiver, receiver)];
        for entry in self
            .cst
            .children(suffix)
            .filter(|child| self.rule(*child) == Some(Rule::FieldProjectionEntry))
        {
            let mut names = self
                .cst
                .children(entry)
                .filter(|child| matches!(self.cst.get(*child), Node::Token(Token::Identifier, _)));
            let from = names.next().ok_or(())?;
            let to = names.next().unwrap_or(from);
            inputs.push(Input::with(Role::Name, from, Mode::Name));
            inputs.push(Input::with(Role::Target, to, Mode::Name));
        }
        let _ = node;
        Ok(Shape::Node(HirKind::FieldProjection, inputs))
    }
}
