use super::*;
use crate::mir::TypeOperation;

impl Lower<'_> {
    pub(super) fn type_term(&self, node: NodeRef) -> Result<Shape, ()> {
        match self.rule(node) {
            Some(Rule::Expression | Rule::Primary | Rule::Braced) => {
                let [inner] = self.operands(node)?;
                Ok(Shape::Alias(inner, Mode::TypeTerm))
            }
            Some(Rule::VariableExpr) => self.expr(node),
            Some(Rule::StaticPathExpr | Rule::StaticPath) => self.expr(node),
            Some(Rule::DotPostfixExpr) => {
                if self.child(node, Rule::MetadataSuffix).is_ok()
                    || self.child(node, Rule::PostfixIntrinsicSuffix).is_ok()
                {
                    return Err(self.invalid_type(node));
                }
                let suffix = self.child(node, Rule::ProjectionSuffix)?;
                if self.token(suffix, Token::Identifier).is_none() {
                    return Err(self.invalid_type(node));
                }
                self.expr(node)
            }
            Some(Rule::ParenExpr) => {
                let items = self.expressions(node);
                if items.len() == 1 && self.token(node, Token::Comma).is_none() {
                    return Ok(Shape::Alias(items[0], Mode::TypeTerm));
                }
                let operation = if items.is_empty() {
                    TypeOperation::Unit
                } else {
                    TypeOperation::Tuple
                };
                Ok(Shape::Desugared(
                    HirKind::TypeOperation(operation),
                    items
                        .into_iter()
                        .map(|node| Input::with(Role::Argument, node, Mode::Type))
                        .collect(),
                ))
            }
            Some(Rule::CallExpr) => {
                let [callee] = self.operands(node)?;
                let mut inputs = vec![Input::expr(Role::Callee, callee)];
                if let Ok(args) = self.child(node, Rule::Arguments) {
                    inputs.extend(
                        self.expressions(args)
                            .into_iter()
                            .map(|node| Input::with(Role::Argument, node, Mode::TypeArgument)),
                    );
                }
                Ok(Shape::Node(HirKind::Call, inputs))
            }
            Some(Rule::FunctionContract | Rule::UnitContract) => self.contract_term(node),
            _ => Err(self.invalid_type(node)),
        }
    }

    pub(super) fn type_argument(&self, node: NodeRef) -> Result<Shape, ()> {
        match self.rule(node) {
            Some(Rule::Expression | Rule::Primary | Rule::Braced) => {
                let [inner] = self.operands(node)?;
                Ok(Shape::Alias(inner, Mode::TypeArgument))
            }
            Some(Rule::ArrayExpr) => Ok(Shape::Node(
                HirKind::Array,
                self.expressions(node)
                    .into_iter()
                    .map(|node| Input::with(Role::Item, node, Mode::Type))
                    .collect(),
            )),
            _ => Ok(Shape::Alias(node, Mode::Type)),
        }
    }

    fn invalid_type(&self, node: NodeRef) {
        self.error(node,
            "a static type requires a type declaration, constructor or family; computed metadata cannot become a type")
    }
}
