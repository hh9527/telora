use super::*;
use crate::mir::TypeOperation;

impl Lower<'_> {
    pub(super) fn contract(&self, node: NodeRef) -> Result<Shape, Diagnostic> {
        match self.rule(node) {
            Some(Rule::Contract) => {
                let inner = self
                    .cst
                    .children(node)
                    .find(|child| self.rule(*child).is_some())
                    .ok_or_else(|| self.error(node, "empty contract"))?;
                Ok(Shape::Alias(inner, Mode::Contract))
            }
            Some(Rule::FunctionContract | Rule::UnitContract) => Ok(Shape::Alias(node, Mode::Type)),
            Some(Rule::ContractExpr) => {
                let path = self.child(node, Rule::ContractPath)?;
                let last = self
                    .cst
                    .children(path)
                    .filter(|child| {
                        matches!(self.cst.get(*child), Node::Token(Token::Identifier, _))
                    })
                    .last()
                    .ok_or_else(|| self.error(node, "contract has no name"))?;
                let args = self.contract_parts(node);
                if args.is_empty() {
                    return Ok(Shape::Alias(path, Mode::Path(last)));
                }
                let mut inputs = vec![Input::with(Role::Callee, path, Mode::Path(last))];
                inputs.extend(
                    args.into_iter()
                        .map(|node| Input::with(Role::Argument, node, Mode::Contract)),
                );
                Ok(Shape::Node(HirKind::Call, inputs))
            }
            Some(Rule::ContractArray) => Ok(Shape::Node(
                HirKind::Array,
                self.contract_parts(node)
                    .into_iter()
                    .map(|node| Input::with(Role::Item, node, Mode::Contract))
                    .collect(),
            )),
            _ => Err(self.error(node, "invalid contract")),
        }
    }

    pub(super) fn path(&self, node: NodeRef, last: NodeRef) -> Result<Shape, Diagnostic> {
        let previous = self
            .cst
            .children(node)
            .take_while(|child| *child != last)
            .filter(|child| matches!(self.cst.get(*child), Node::Token(Token::Identifier, _)))
            .last();
        if let Some(previous) = previous {
            Ok(Shape::Node(
                HirKind::Field,
                vec![
                    Input::with(Role::Receiver, node, Mode::Path(previous)),
                    Input::with(Role::Name, last, Mode::Name),
                ],
            ))
        } else {
            Ok(Shape::Node(
                HirKind::Variable(self.text(last).into_owned()),
                vec![],
            ))
        }
    }

    pub(super) fn contract_term(&self, node: NodeRef) -> Result<Shape, Diagnostic> {
        let parts = self.contract_parts(node);
        let operation = match self.rule(node) {
            Some(Rule::FunctionContract) => {
                if parts.is_empty() {
                    return Err(self.error(node, "function contract has no result"));
                }
                TypeOperation::Function
            }
            Some(Rule::UnitContract) => {
                if parts.len() == 1 && self.token(node, Token::Comma).is_none() {
                    return Ok(Shape::Alias(parts[0], Mode::Contract));
                }
                if parts.is_empty() {
                    TypeOperation::Unit
                } else {
                    TypeOperation::Tuple
                }
            }
            _ => unreachable!("contract type operation"),
        };
        Ok(Shape::Desugared(
            HirKind::TypeOperation(operation),
            parts
                .into_iter()
                .map(|node| Input::with(Role::Argument, node, Mode::Contract))
                .collect(),
        ))
    }

    fn contract_parts(&self, node: NodeRef) -> Vec<NodeRef> {
        self.cst
            .children(node)
            .filter(|child| {
                matches!(
                    self.rule(*child),
                    Some(
                        Rule::Contract
                            | Rule::ContractExpr
                            | Rule::FunctionContract
                            | Rule::UnitContract
                            | Rule::ContractArray
                    )
                )
            })
            .collect()
    }
}
