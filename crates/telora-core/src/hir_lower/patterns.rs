use super::*;

impl Lower<'_> {
    pub(super) fn is_pattern(&self, node: NodeRef) -> bool {
        matches!(
            self.cst.get(node),
            Node::Token(
                Token::Identifier | Token::Placeholder | Token::Int | Token::Float,
                _
            )
        ) || matches!(
            self.rule(node),
            Some(
                Rule::Pattern
                    | Rule::IdentifierPattern
                    | Rule::ConstructorPattern
                    | Rule::TuplePattern
                    | Rule::StructPattern
                    | Rule::IntPattern
                    | Rule::FloatPattern
                    | Rule::StringPattern
            )
        )
    }

    pub(super) fn pattern(&self, node: NodeRef) -> Result<Shape, ()> {
        if self.rule(node) == Some(Rule::Pattern) {
            let child = self
                .cst
                .children(node)
                .find(|child| self.rule(*child).is_some())
                .ok_or(())?;
            return Ok(Shape::Alias(child, Mode::Pattern));
        }
        let kind = match self.cst.get(node) {
            Node::Token(Token::Identifier, _) => HirKind::PatternName(self.text(node).into_owned()),
            Node::Token(Token::Placeholder, _) => HirKind::Wildcard,
            Node::Token(Token::Int, _) => HirKind::Int(
                self.text(node)
                    .parse()
                    .map_err(|_| self.error(node, "invalid Int pattern"))?,
            ),
            Node::Token(Token::Float, _) => {
                let value: f64 = self
                    .text(node)
                    .parse()
                    .map_err(|_| self.error(node, "invalid Float literal"))?;
                if !value.is_finite() {
                    return Err(self.error(node, "Float literal must be finite"));
                }
                HirKind::Float(value)
            }
            Node::Rule(Rule::IdentifierPattern | Rule::IntPattern | Rule::FloatPattern, _) => {
                let child = self
                    .cst
                    .children(node)
                    .find(|child| self.is_pattern(*child))
                    .ok_or(())?;
                return Ok(Shape::Alias(child, Mode::Pattern));
            }
            Node::Rule(Rule::StringPattern, _) => {
                HirKind::String(self.plain_string(self.child(node, Rule::StringLiteral)?)?)
            }
            Node::Rule(Rule::ConstructorPattern, _) => {
                let last = self
                    .cst
                    .children(node)
                    .filter(|child| {
                        matches!(self.cst.get(*child), Node::Token(Token::Identifier, _))
                    })
                    .last()
                    .ok_or(())?;
                let mut inputs = vec![Input::with(Role::Callee, node, Mode::Path(last))];
                if let Some(open) = self.token(node, Token::LParen) {
                    let payload = self
                        .cst
                        .children(node)
                        .find(|child| {
                            self.is_pattern(*child)
                                && self.cst.span(*child).start >= self.cst.span(open).end
                        })
                        .ok_or(())?;
                    inputs.push(Input::with(Role::Pattern, payload, Mode::Pattern));
                }
                return Ok(Shape::Node(HirKind::ConstructorPattern, inputs));
            }
            Node::Rule(Rule::TuplePattern, _) => {
                return Ok(Shape::Node(
                    HirKind::TuplePattern,
                    self.cst
                        .children(node)
                        .filter(|child| self.is_pattern(*child))
                        .map(|node| Input::with(Role::Item, node, Mode::Pattern))
                        .collect(),
                ));
            }
            Node::Rule(Rule::StructPattern, _) => {
                return Ok(Shape::Node(
                    HirKind::StructPattern,
                    self.cst
                        .children(node)
                        .filter(|child| self.rule(*child) == Some(Rule::StructPatternField))
                        .map(|node| Input::with(Role::Field, node, Mode::PatternField))
                        .collect(),
                ));
            }
            _ => return Err(self.error(node, "unexpected pattern rule")),
        };
        Ok(Shape::Node(kind, vec![]))
    }

    pub(super) fn pattern_field(&self, node: NodeRef) -> Result<Shape, ()> {
        let name = self.required_token(node, Token::Identifier)?;
        let pattern = self
            .cst
            .children(node)
            .find(|child| *child != name && self.is_pattern(*child))
            .unwrap_or(name);
        Ok(Shape::Node(
            HirKind::PatternField,
            vec![
                Input::with(Role::Name, name, Mode::Name),
                Input::with(Role::Pattern, pattern, Mode::Pattern),
            ],
        ))
    }

    pub(super) fn arm(&self, node: NodeRef) -> Result<Shape, ()> {
        let arrow = self.token(node, Token::FatArrow);
        let pattern = self
            .cst
            .children(node)
            .find(|child| self.is_pattern(*child) && arrow.is_none_or(|arrow| child.0 < arrow.0));
        let value = self
            .expressions(node)
            .into_iter()
            .find(|child| arrow.is_none_or(|arrow| child.0 > arrow.0));
        let pattern = match pattern {
            Some(pattern) => Input::with(Role::Pattern, pattern, Mode::Pattern),
            None => self.synthetic(Role::Pattern, node, HirKind::Missing, vec![]),
        };
        let mut inputs = vec![pattern];
        if let Some(guard_start) = self.token(node, Token::If) {
            let guard = self.expressions(node).into_iter().find(|child| {
                child.0 > guard_start.0 && arrow.is_some_and(|arrow| child.0 < arrow.0)
            });
            inputs.push(self.optional_expression(Role::Guard, node, guard));
        }
        inputs.push(self.optional_expression(Role::Value, node, value));
        Ok(Shape::Node(
            HirKind::MatchArm { irrefutable: false },
            inputs,
        ))
    }
}
