use super::*;
use crate::syntax::kinds::{BinaryOperator as B, UnaryOperator};

impl Lower<'_> {
    pub(super) fn expr(&self, node: NodeRef) -> Result<Shape, ()> {
        let shape = match self.rule(node) {
            Some(Rule::StringExpr) => return self.string(node),
            Some(Rule::BytesExpr) => return self.bytes(node),
            Some(Rule::NamedIntrinsic | Rule::InterpreterIntrinsic) => return self.intrinsic(node),
            Some(Rule::LegacyInterpreterExpr) => {
                return Err(self.error(
                    node,
                    "interpreter(...) has been replaced by interpreter!(...)",
                ));
            }
            Some(Rule::SectionExpr) => return self.section(node),
            Some(Rule::TypeApplyExpr) => return self.type_apply(node),
            Some(Rule::DictExpr) => return self.record(node),
            Some(Rule::IfExpr | Rule::IfLetExpr | Rule::MatchExpr) => return self.control(node),
            Some(Rule::FunctionContract) => Shape::Alias(node, Mode::Type),
            Some(Rule::Expression | Rule::Primary | Rule::Braced) => {
                let [inner] = self.operands(node)?;
                Shape::Alias(inner, Mode::Expression)
            }
            Some(Rule::IntExpr) => {
                let token = self.token(node, Token::Int).ok_or(())?;
                let value = self
                    .text(token)
                    .parse()
                    .map_err(|_| self.error(node, "Int literal is outside the i64 range"))?;
                Shape::Node(HirKind::Int(value), vec![])
            }
            Some(Rule::FloatExpr) => {
                let token = self.token(node, Token::Float).ok_or(())?;
                let value: f64 = self
                    .text(token)
                    .parse()
                    .map_err(|_| self.error(node, "invalid Float literal"))?;
                if !value.is_finite() {
                    return Err(self.error(node, "Float literal must be finite"));
                }
                Shape::Node(HirKind::Float(value), vec![])
            }
            Some(Rule::VariableExpr) => {
                let token = self.token(node, Token::Identifier).ok_or(())?;
                Shape::Node(HirKind::Variable(self.text(token).into_owned()), vec![])
            }
            Some(Rule::ArrayExpr | Rule::ParenExpr) => {
                let items = self.expressions(node);
                let array = self.rule(node) == Some(Rule::ArrayExpr);
                if !array
                    && items.len() == 1
                    && self.token(node, Token::Comma).is_none()
                    && self.rule(items[0]) != Some(Rule::SpreadExpr)
                {
                    Shape::Alias(items[0], Mode::Expression)
                } else {
                    Shape::Node(
                        if array {
                            HirKind::Array
                        } else {
                            HirKind::Tuple
                        },
                        items
                            .into_iter()
                            .map(|node| Input::expr(Role::Item, node))
                            .collect(),
                    )
                }
            }
            Some(Rule::SpreadExpr | Rule::PropagateExpr | Rule::ReturnExpr | Rule::UnaryExpr) => {
                let operand = self.expressions(node).into_iter().next();
                let (kind, role) = match self.rule(node).unwrap() {
                    Rule::SpreadExpr => (HirKind::Spread, Role::Operand),
                    Rule::PropagateExpr => (HirKind::Propagate, Role::Operand),
                    Rule::ReturnExpr => (HirKind::Return, Role::Value),
                    _ => (
                        HirKind::Unary(if self.token(node, Token::Minus).is_some() {
                            UnaryOperator::Negate
                        } else {
                            UnaryOperator::Not
                        }),
                        Role::Operand,
                    ),
                };
                Shape::Node(kind, vec![self.optional_expression(role, node, operand)])
            }
            Some(Rule::BinaryExpr) => {
                let operands = self.expressions(node);
                let operator = self.binary_operator(node)?;
                if comparison(operator)
                    && operands.iter().copied().any(|child| {
                        self.rule(child) == Some(Rule::BinaryExpr)
                            && self.binary_operator(child).is_ok_and(comparison)
                    })
                {
                    return Err(self.error(
                        node,
                        "comparison operators do not associate; add parentheses",
                    ));
                }
                Shape::Node(
                    HirKind::Binary(operator),
                    vec![
                        self.optional_expression(Role::Left, node, operands.first().copied()),
                        self.optional_expression(Role::Right, node, operands.get(1).copied()),
                    ],
                )
            }
            Some(Rule::IndexExpr) => {
                let operands = self.expressions(node);
                Shape::Node(
                    HirKind::Index,
                    vec![
                        self.optional_expression(Role::Receiver, node, operands.first().copied()),
                        self.optional_expression(Role::Index, node, operands.get(1).copied()),
                    ],
                )
            }
            Some(Rule::DotPostfixExpr) => {
                let [receiver] = self.operands(node)?;
                if let Ok(suffix) = self.child(node, Rule::ProjectionSuffix) {
                    if let Some(name) = self.token(suffix, Token::Identifier) {
                        Shape::Node(
                            HirKind::Field,
                            vec![
                                Input::expr(Role::Receiver, receiver),
                                Input::with(Role::Name, name, Mode::Name),
                            ],
                        )
                    } else {
                        let index = self.token(suffix, Token::Int).ok_or(())?;
                        let index = self.text(index).parse().map_err(|_| {
                            self.error(index, "tuple projection index is too large")
                        })?;
                        Shape::Node(
                            HirKind::TupleProjection(index),
                            vec![Input::expr(Role::Receiver, receiver)],
                        )
                    }
                } else if self.child(node, Rule::MetadataSuffix).is_ok() {
                    Shape::Node(
                        HirKind::TypeMetadata,
                        vec![Input::with(Role::Operand, receiver, Mode::Type)],
                    )
                } else if let Ok(suffix) = self.child(node, Rule::FieldProjectionSuffix) {
                    return self.field_projection(node, receiver, suffix);
                } else if let Ok(suffix) = self.child(node, Rule::PostfixIntrinsicSuffix) {
                    return self.postfix_intrinsic(node, receiver, suffix);
                } else {
                    Shape::Node(
                        HirKind::Field,
                        vec![
                            Input::expr(Role::Receiver, receiver),
                            self.synthetic(Role::Name, node, HirKind::Missing, vec![]),
                        ],
                    )
                }
            }
            Some(Rule::CallExpr) => {
                let callee = self.expressions(node).into_iter().next();
                let mut inputs = vec![self.optional_expression(Role::Callee, node, callee)];
                if let Ok(args) = self.child(node, Rule::Arguments) {
                    inputs.extend(
                        self.expressions(args)
                            .into_iter()
                            .map(|node| Input::expr(Role::Argument, node)),
                    );
                }
                Shape::Node(HirKind::Call, inputs)
            }
            Some(Rule::PipelineExpr) => {
                let [argument, callee] = self.operands(node)?;
                Shape::Desugared(
                    HirKind::Call,
                    vec![
                        Input::expr(Role::Callee, callee),
                        Input::expr(Role::Argument, argument),
                    ],
                )
            }
            Some(Rule::Block) => Shape::Alias(node, Mode::Body),
            Some(Rule::DoExpr) => Shape::Alias(self.child(node, Rule::Block)?, Mode::Body),
            Some(Rule::Closure) => return self.closure(node),
            _ => {
                return Err(self.error(
                    node,
                    format!(
                        "CST-to-HIR lowering is not implemented for {:?}",
                        self.cst.get(node)
                    ),
                ));
            }
        };
        Ok(shape)
    }

    fn binary_operator(&self, node: NodeRef) -> Result<B, ()> {
        [
            (Token::Plus, B::Add),
            (Token::Minus, B::Subtract),
            (Token::Star, B::Multiply),
            (Token::Slash, B::Divide),
            (Token::Percent, B::Remainder),
            (Token::Less, B::LessThan),
            (Token::LessEqual, B::LessThanOrEqual),
            (Token::Greater, B::GreaterThan),
            (Token::GreaterEqual, B::GreaterThanOrEqual),
            (Token::EqualEqual, B::Equal),
            (Token::BangEqual, B::NotEqual),
            (Token::StructUpdate, B::StructUpdate),
            (Token::BitAnd, B::BitAnd),
            (Token::BitOr, B::BitOr),
            (Token::BitXor, B::BitXor),
            (Token::AndAnd, B::And),
            (Token::OrOr, B::Or),
        ]
        .into_iter()
        .find_map(|(token, kind)| self.token(node, token).map(|_| kind))
        .ok_or(())
    }
}

fn comparison(operator: B) -> bool {
    matches!(
        operator,
        B::LessThan
            | B::LessThanOrEqual
            | B::GreaterThan
            | B::GreaterThanOrEqual
            | B::Equal
            | B::NotEqual
    )
}
