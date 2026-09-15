use super::*;

impl Lower<'_> {
    pub(super) fn control(&self, node: NodeRef) -> Result<Shape, ()> {
        if self.rule(node) == Some(Rule::MatchExpr) {
            let value = self.expressions(node).into_iter().next();
            let mut inputs = vec![self.optional_expression(Role::Value, node, value)];
            inputs.extend(
                self.cst
                    .children(node)
                    .filter(|child| self.rule(*child) == Some(Rule::MatchArm))
                    .map(|node| Input::with(Role::Arm, node, Mode::Arm)),
            );
            return Ok(Shape::Node(HirKind::Match, inputs));
        }
        let then = self.child(node, Rule::Block)?;
        let else_token = self.required_token(node, Token::Else)?;
        let alternative = self
            .expressions(node)
            .into_iter()
            .find(|child| child.0 > else_token.0)
            .ok_or(())?;
        let alternative = if self.rule(alternative) == Some(Rule::Block) {
            Input::with(Role::Else, alternative, Mode::Body)
        } else {
            self.synthetic(
                Role::Else,
                alternative,
                HirKind::Block,
                vec![Input::expr(Role::Result, alternative)],
            )
        };
        let if_let = self.rule(node) == Some(Rule::IfLetExpr);
        let mut inputs = vec![];
        if if_let {
            let pattern = self
                .cst
                .children(node)
                .find(|child| self.rule(*child).is_some() && self.is_pattern(*child))
                .ok_or(())?;
            inputs.push(Input::with(Role::Pattern, pattern, Mode::Pattern));
        }
        let value = self
            .expressions(node)
            .into_iter()
            .find(|child| child.0 < then.0);
        inputs.push(self.optional_expression(
            if if_let { Role::Value } else { Role::Condition },
            node,
            value,
        ));
        inputs.push(Input::with(Role::Then, then, Mode::Body));
        inputs.push(alternative);
        Ok(Shape::Node(
            if if_let { HirKind::IfLet } else { HirKind::If },
            inputs,
        ))
    }
}
