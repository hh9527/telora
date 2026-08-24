impl<'a> Lowerer<'a> {
    fn expression(&self, node: NodeRef) -> Result<Expr, Diagnostic> {
        if let Node::Token(token, _) = self.cst.get(node) {
            let location = self.location(node);
            let inner = match token {
                Token::Int => ExprKind::Int(
                    self.text(node)
                        .parse()
                        .map_err(|_| self.error(node, "Int literal is outside the i64 range"))?,
                ),
                Token::Float => ExprKind::Float(
                    parse_float_literal(&self.text(node))
                        .map_err(|message| self.error(node, message))?,
                ),
                Token::Bytes => ExprKind::Bytes(self.decode_telora_string(node)?.into_bytes()),
                Token::Atom => ExprKind::Atom(self.text(node).trim_start_matches('\'').to_owned()),
                Token::Identifier => ExprKind::Variable(self.identifier(node)),
                _ => return Err(self.error(node, "expected expression token")),
            };
            return Ok(located(inner, location));
        }
        let Some(rule) = self.rule(node) else {
            return Err(self.error(node, "expected expression"));
        };
        if matches!(rule, Rule::Expression | Rule::Primary | Rule::Braced) {
            return self.expression(
                self.first_rule(node)
                    .ok_or_else(|| self.error(node, "empty expression"))?,
            );
        }
        let location = self.location(node);
        let rules = self.rule_children(node).collect::<Vec<_>>();
        let inner = match rule {
            Rule::IntExpr => ExprKind::Int(
                self.text(self.first_token(node, Token::Int)?)
                    .parse()
                    .map_err(|_| self.error(node, "Int literal is outside the i64 range"))?,
            ),
            Rule::FloatExpr => {
                let token = self.first_token(node, Token::Float)?;
                ExprKind::Float(
                    parse_float_literal(&self.text(token))
                        .map_err(|message| self.error(token, message))?,
                )
            }
            Rule::StringExpr => return self.string_expression(node),
            Rule::BytesExpr => ExprKind::Bytes(
                self.decode_telora_string(self.first_token(node, Token::Bytes)?)?
                    .into_bytes(),
            ),
            Rule::AtomExpr => ExprKind::Atom(
                self.text(self.first_token(node, Token::Atom)?)
                    .trim_start_matches('\'')
                    .to_owned(),
            ),
            Rule::VariableExpr => {
                ExprKind::Variable(self.identifier(self.first_token(node, Token::Identifier)?))
            }
            Rule::ArrayExpr => ExprKind::Array(self.expression_children(node)?),
            Rule::SpreadExpr => {
                let operand = self
                    .children(node)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(node, "spread has no operand"))?;
                ExprKind::Spread(Box::new(self.expression(operand)?))
            }
            Rule::ParenExpr => {
                let items = self.expression_children(node)?;
                if items.len() == 1 && self.token_children(node, Token::Comma).next().is_none() {
                    return Ok(items.into_iter().next().unwrap());
                }
                ExprKind::Tuple(items)
            }
            Rule::DictExpr => {
                let mut fields = Vec::new();
                for field in rules.iter().copied().filter(|child| {
                    matches!(self.rule(*child), Some(Rule::DictField | Rule::SpreadExpr))
                }) {
                    if self.rule(field) == Some(Rule::SpreadExpr) {
                        fields.push(located(
                            DictFieldKind {
                                decorators: Vec::new(),
                                name: None,
                                value: self.expression(field)?,
                            },
                            self.location(field),
                        ));
                        continue;
                    }
                    let key = self
                        .children(field)
                        .find(|child| {
                            matches!(self.cst.get(*child), Node::Token(Token::Identifier, _))
                                || self.rule(*child) == Some(Rule::StringLiteral)
                        })
                        .ok_or_else(|| self.error(field, "Dict field has no key"))?;
                    let name = if self.rule(key) == Some(Rule::StringLiteral) {
                        located(
                            self.plain_string(key, "Dict field name")?,
                            self.location(key),
                        )
                    } else {
                        self.identifier(key)
                    };
                    let decorators = self.decorators(field)?;
                    if !decorators.is_empty() {
                        return Err(self.error(
                            field,
                            "decorators are only supported on concrete nominal type declarations; Dict fields do not have property identity",
                        ));
                    }
                    let value = if let Ok(colon) = self.first_token(field, Token::Colon) {
                        let value = self
                            .children(field)
                            .find(|child| {
                                self.is_expression(*child)
                                    && self.cst.span(*child).start > self.cst.span(colon).start
                            })
                            .ok_or_else(|| self.error(field, "Dict field has no value"))?;
                        self.expression(value)?
                    } else {
                        if !decorators.is_empty() {
                            return Err(self
                                .error(field, "decorated Dict fields require an explicit value"));
                        }
                        located(ExprKind::Variable(name.clone()), name.location)
                    };
                    fields.push(located(
                        DictFieldKind {
                            decorators,
                            name: Some(name),
                            value,
                        },
                        self.location(field),
                    ));
                }
                ExprKind::Dict(fields)
            }
            Rule::Block => ExprKind::Block(self.block_body(node)?),
            Rule::DoExpr => {
                let block = rules
                    .iter()
                    .find(|child| self.rule(**child) == Some(Rule::Block))
                    .copied()
                    .ok_or_else(|| self.error(node, "do expression has no block"))?;
                ExprKind::Block(self.block_body(block)?)
            }
            Rule::Closure => {
                let parameters = rules
                    .iter()
                    .find(|child| self.rule(**child) == Some(Rule::Parameters))
                    .copied()
                    .ok_or_else(|| self.error(node, "closure has no parameters"))?;
                let block = rules
                    .iter()
                    .find(|child| self.rule(**child) == Some(Rule::Block))
                    .copied()
                    .ok_or_else(|| self.error(node, "closure has no body"))?;
                let result_annotation =
                    self.token_children(node, Token::Arrow)
                        .next()
                        .and_then(|arrow| {
                            rules.iter().copied().find(|child| {
                                self.is_expression(*child)
                                    && self.cst.span(*child).start > self.cst.span(arrow).start
                                    && self.cst.span(*child).end <= self.cst.span(block).start
                            })
                        });
                ExprKind::Closure {
                    parameters: self.parameters(parameters)?,
                    result_annotation: result_annotation
                        .map(|annotation| self.expression(annotation).map(Box::new))
                        .transpose()?,
                    body: self.block_body(block)?,
                }
            }
            Rule::InterpreterIntrinsic | Rule::NamedIntrinsic => {
                return self.lower_contextual_intrinsic(node, &[], None);
            }
            Rule::LegacyInterpreterExpr => {
                return Err(self.error(
                    node,
                    "interpreter(...) has been replaced by interpreter!(...)",
                ));
            }
            Rule::FunctionContract => return self.contract_expression(node),
            Rule::UnaryExpr => ExprKind::Unary {
                operator: if let Some(operator) = self.token_children(node, Token::Minus).next() {
                    located(UnaryOperator::Negate, self.location(operator))
                } else {
                    let operator = self.first_token(node, Token::Bang)?;
                    located(UnaryOperator::Not, self.location(operator))
                },
                operand: Box::new(
                    self.expression(
                        self.children(node)
                            .find(|child| self.is_expression(*child))
                            .ok_or_else(|| self.error(node, "unary expression has no operand"))?,
                    )?,
                ),
            },
            Rule::PropagateExpr => ExprKind::Propagate {
                operand: Box::new(
                    self.expression(
                        rules
                            .iter()
                            .copied()
                            .find(|child| self.is_expression(*child))
                            .ok_or_else(|| self.error(node, "propagation has no operand"))?,
                    )?,
                ),
            },
            Rule::ReturnExpr => ExprKind::Return {
                value: Box::new(
                    self.expression(
                        rules
                            .iter()
                            .copied()
                            .find(|child| self.is_expression(*child))
                            .ok_or_else(|| self.error(node, "return has no value"))?,
                    )?,
                ),
            },
            Rule::BinaryExpr => {
                let is_comparison = |node| {
                    [
                        Token::Less,
                        Token::LessEqual,
                        Token::Greater,
                        Token::GreaterEqual,
                        Token::EqualEqual,
                        Token::BangEqual,
                    ]
                    .into_iter()
                    .any(|token| self.token_children(node, token).next().is_some())
                };
                let comparison = is_comparison(node);
                if comparison
                    && self.children(node).any(|child| {
                        self.rule(child) == Some(Rule::BinaryExpr) && is_comparison(child)
                    })
                {
                    return Err(self.error(
                        node,
                        "comparison operators do not associate; add parentheses",
                    ));
                }
                let values = self.expression_children(node)?;
                let left = values
                    .first()
                    .cloned()
                    .ok_or_else(|| self.error(node, "binary expression has no left operand"))?;
                let right = values
                    .get(1)
                    .cloned()
                    .ok_or_else(|| self.error(node, "binary expression has no right operand"))?;
                let (operator, operator_node) = if let Some(operator) =
                    self.token_children(node, Token::Plus).next()
                {
                    (BinaryOperator::Add, operator)
                } else if let Some(operator) = self.token_children(node, Token::Minus).next() {
                    (BinaryOperator::Subtract, operator)
                } else if let Some(operator) = self.token_children(node, Token::Star).next() {
                    (BinaryOperator::Multiply, operator)
                } else if let Some(operator) = self.token_children(node, Token::Slash).next() {
                    (BinaryOperator::Divide, operator)
                } else if let Some(operator) = self.token_children(node, Token::Percent).next() {
                    (BinaryOperator::Remainder, operator)
                } else if let Some(operator) = self.token_children(node, Token::Less).next() {
                    (BinaryOperator::LessThan, operator)
                } else if let Some(operator) = self.token_children(node, Token::LessEqual).next() {
                    (BinaryOperator::LessThanOrEqual, operator)
                } else if let Some(operator) = self.token_children(node, Token::Greater).next() {
                    (BinaryOperator::GreaterThan, operator)
                } else if let Some(operator) = self.token_children(node, Token::GreaterEqual).next()
                {
                    (BinaryOperator::GreaterThanOrEqual, operator)
                } else if let Some(operator) = self.token_children(node, Token::BangEqual).next() {
                    (BinaryOperator::NotEqual, operator)
                } else if let Some(operator) = self.token_children(node, Token::BitAnd).next() {
                    (BinaryOperator::BitAnd, operator)
                } else if let Some(operator) = self.token_children(node, Token::BitOr).next() {
                    (BinaryOperator::BitOr, operator)
                } else if let Some(operator) = self.token_children(node, Token::BitXor).next() {
                    (BinaryOperator::BitXor, operator)
                } else if let Some(operator) = self.token_children(node, Token::AndAnd).next() {
                    (BinaryOperator::And, operator)
                } else if let Some(operator) = self.token_children(node, Token::OrOr).next() {
                    (BinaryOperator::Or, operator)
                } else {
                    (
                        BinaryOperator::Equal,
                        self.first_token(node, Token::EqualEqual)?,
                    )
                };
                ExprKind::Binary {
                    operator: located(operator, self.location(operator_node)),
                    left: Box::new(left),
                    right: Box::new(right),
                }
            }
            Rule::DotPostfixExpr => {
                let receiver = self
                    .children(node)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(node, "dot postfix expression has no receiver"))?;
                let receiver_expression = self.expression(receiver)?;
                let suffix = self
                    .rule_children(node)
                    .find(|child| {
                        matches!(
                            self.rule(*child),
                            Some(Rule::PostfixIntrinsicSuffix | Rule::ProjectionSuffix)
                        )
                    })
                    .ok_or_else(|| self.error(node, "dot postfix expression has no suffix"))?;
                if self.rule(suffix) == Some(Rule::PostfixIntrinsicSuffix) {
                    return self.lower_postfix_intrinsic(
                        receiver,
                        receiver_expression,
                        suffix,
                        node,
                    );
                }
                let receiver = Box::new(receiver_expression);
                if let Some(field) = self.token_children(suffix, Token::Identifier).last() {
                    ExprKind::Field {
                        receiver,
                        field: self.identifier(field),
                    }
                } else {
                    let index_token = self.first_token(suffix, Token::Int)?;
                    let index = self.text(index_token).parse::<usize>().map_err(|_| {
                        self.error(index_token, "tuple projection index is too large")
                    })?;
                    ExprKind::TupleProjection {
                        receiver,
                        index: located(index, self.location(index_token)),
                    }
                }
            }
            Rule::IndexExpr => {
                let mut values = self.expression_children(node)?;
                if values.len() != 2 {
                    return Err(self.error(node, "array index requires a receiver and an index"));
                }
                let index = values.pop().expect("two expressions");
                let receiver = values.pop().expect("two expressions");
                ExprKind::Index {
                    receiver: Box::new(receiver),
                    index: Box::new(index),
                }
            }
            Rule::CallExpr => {
                let callee_node = self
                    .children(node)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(node, "call has no callee"))?;
                let callee = self.expression(callee_node)?;
                let arguments = rules
                    .iter()
                    .find(|child| self.rule(**child) == Some(Rule::Arguments))
                    .map_or(Ok(Vec::new()), |args| self.expression_children(*args))?;
                if let ExprKind::TypeApply {
                    callee: applied,
                    arguments: type_arguments,
                } = &callee.value
                    && let ExprKind::Field { receiver, field } = &applied.value
                    && field.value == "project"
                    && let [type_argument] = type_arguments.as_slice()
                    && let TypeArgumentKind::Explicit(target) = &type_argument.value
                    && let [value] = arguments.as_slice()
                {
                    ExprKind::DynProject {
                        namespace: receiver.clone(),
                        target: Box::new(target.clone()),
                        value: Box::new(value.clone()),
                    }
                } else {
                    ExprKind::Call {
                        callee: Box::new(callee),
                        arguments,
                    }
                }
            }
            Rule::TypeApplyExpr => {
                let callee_node = self
                    .children(node)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(node, "type application has no callee"))?;
                let arguments = rules
                    .iter()
                    .find(|child| self.rule(**child) == Some(Rule::TypeArguments))
                    .map_or(Ok(Vec::new()), |args| self.type_arguments(*args))?;
                ExprKind::TypeApply {
                    callee: Box::new(self.expression(callee_node)?),
                    arguments,
                }
            }
            Rule::SectionExpr => {
                let callee_node = self
                    .children(node)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(node, "call section has no callee"))?;
                let callee = self.expression(callee_node)?;
                let arguments_node = rules
                    .iter()
                    .find(|child| self.rule(**child) == Some(Rule::SectionArguments))
                    .copied();
                return self.section_expression(callee, arguments_node, node, location);
            }
            Rule::PipelineExpr => {
                let values = self.expression_children(node)?;
                return Ok(elaborate_pipeline(
                    location,
                    values[0].clone(),
                    values[1].clone(),
                ));
            }
            Rule::IfExpr => {
                let condition_node = self
                    .children(node)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(node, "if has no condition"))?;
                let condition = self.expression(condition_node)?;
                let blocks = rules
                    .iter()
                    .filter(|child| self.rule(**child) == Some(Rule::Block))
                    .copied()
                    .collect::<Vec<_>>();
                let else_branch = self.ctrl_block(node, &blocks, &rules)?;
                ExprKind::If {
                    condition: Box::new(condition),
                    then_branch: self.block_body(blocks[0])?,
                    else_branch,
                }
            }
            Rule::IfLetExpr => {
                let pattern = rules
                    .iter()
                    .copied()
                    .find(|child| self.is_pattern(*child))
                    .ok_or_else(|| self.error(node, "if let has no pattern"))?;
                let value = rules
                    .iter()
                    .copied()
                    .find(|child| {
                        self.is_expression(*child) && self.rule(*child) != Some(Rule::Block)
                    })
                    .ok_or_else(|| self.error(node, "if let has no value"))?;
                let blocks = rules
                    .iter()
                    .copied()
                    .filter(|child| self.rule(*child) == Some(Rule::Block))
                    .collect::<Vec<_>>();
                ExprKind::IfLet {
                    pattern: self.pattern(pattern)?,
                    value: Box::new(self.expression(value)?),
                    then_branch: self.block_body(blocks[0])?,
                    else_branch: self.ctrl_block(node, &blocks, &rules)?,
                }
            }
            Rule::MatchExpr => {
                let value_node = self
                    .children(node)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(node, "match has no value"))?;
                let value = self.expression(value_node)?;
                let arms = rules
                    .iter()
                    .copied()
                    .filter(|child| self.rule(*child) == Some(Rule::MatchArm))
                    .map(|arm| self.match_arm(arm))
                    .collect::<Result<Vec<_>, _>>()?;
                ExprKind::Match {
                    value: Box::new(value),
                    arms,
                }
            }
            _ => return Err(self.error(node, format!("unexpected expression rule {rule:?}"))),
        };
        Ok(located(inner, location))
    }

    fn ctrl_block(
        &self,
        node: NodeRef,
        blocks: &[NodeRef],
        rules: &[NodeRef],
    ) -> Result<Block, Diagnostic> {
        if let Some(block) = blocks.get(1) {
            return self.block_body(*block);
        }
        let nested = rules
            .iter()
            .copied()
            .find(|child| {
                matches!(
                    self.rule(*child),
                    Some(Rule::IfExpr | Rule::IfLetExpr | Rule::MatchExpr | Rule::ReturnExpr)
                )
            })
            .ok_or_else(|| self.error(node, "control-flow expression has no ctrl_block"))?;
        let nested = self.expression(nested)?;
        Ok(located(
            BlockKind {
                bindings: Vec::new(),
                result: Box::new(nested.clone()),
            },
            nested.location,
        ))
    }

    fn lower_contextual_intrinsic(
        &self,
        node: NodeRef,
        type_parameters: &[Identifier],
        contract: Option<&Expr>,
    ) -> Result<Expr, Diagnostic> {
        match self.rule(node) {
            Some(Rule::InterpreterIntrinsic) => {
                self.lower_interpreter(node, type_parameters, contract)
            }
            Some(Rule::NamedIntrinsic) => {
                let name = self
                    .token_children(node, Token::Identifier)
                    .next()
                    .ok_or_else(|| self.error(node, "contextual intrinsic has no name"))?;
                let name_text = self.text(name);
                let bang = self.first_token(node, Token::Bang)?;
                let argument_nodes = self
                    .children(node)
                    .filter(|child| {
                        self.is_expression(*child)
                            && self.cst.span(*child).start > self.cst.span(bang).end
                    })
                    .collect::<Vec<_>>();
                self.lower_named_intrinsic(&name_text, name, &argument_nodes, node)
            }
            _ => Err(self.error(node, "contextual intrinsic has no supported name")),
        }
    }

    fn lower_postfix_intrinsic(
        &self,
        receiver_node: NodeRef,
        receiver: Expr,
        suffix: NodeRef,
        invocation: NodeRef,
    ) -> Result<Expr, Diagnostic> {
        let name = self
            .token_children(suffix, Token::Identifier)
            .next()
            .ok_or_else(|| self.error(suffix, "postfix contextual intrinsic has no name"))?;
        let name_text = self.text(name);
        let arguments_node = self
            .rule_children(suffix)
            .find(|child| self.rule(*child) == Some(Rule::Arguments))
            .ok_or_else(|| self.error(suffix, "postfix contextual intrinsic has no arguments"))?;
        let mut argument_nodes = vec![receiver_node];
        argument_nodes.extend(
            self.children(arguments_node)
                .filter(|child| self.is_expression(*child)),
        );
        self.lower_named_intrinsic_with_receiver(
            &name_text,
            name,
            &argument_nodes,
            Some(receiver),
            invocation,
        )
    }

    fn lower_named_intrinsic(
        &self,
        name: &str,
        name_node: NodeRef,
        argument_nodes: &[NodeRef],
        invocation: NodeRef,
    ) -> Result<Expr, Diagnostic> {
        self.lower_named_intrinsic_with_receiver(name, name_node, argument_nodes, None, invocation)
    }

    fn lower_named_intrinsic_with_receiver(
        &self,
        name: &str,
        name_node: NodeRef,
        argument_nodes: &[NodeRef],
        receiver: Option<Expr>,
        invocation: NodeRef,
    ) -> Result<Expr, Diagnostic> {
        if !matches!(
            name,
            "panic"
                | "dbg"
                | "ty"
                | "cast"
                | "should_ok"
                | "must_ok"
                | "try_unwrap"
                | "unwrap"
                | "fail"
        ) {
            return if matches!(name, "file" | "line") {
                Err(self.error(
                    name_node,
                    format!("{name}! is reserved but not implemented"),
                ))
            } else {
                Err(self.error(name_node, format!("unknown contextual intrinsic {name}!")))
            };
        }
        if name == "dbg" {
            return self.lower_debug(argument_nodes, receiver, invocation);
        }
        let mut arguments = Vec::with_capacity(argument_nodes.len());
        for (index, argument) in argument_nodes.iter().copied().enumerate() {
            if index == 0
                && let Some(receiver) = receiver.clone()
            {
                arguments.push(receiver);
            } else {
                arguments.push(self.expression(argument)?);
            }
        }
        if name == "ty" {
            if arguments.len() != 2 {
                return Err(self.error(
                    invocation,
                    format!(
                        "ty! expects a value and a Type, found {} arguments",
                        arguments.len()
                    ),
                ));
            }
            let mut arguments = arguments.into_iter();
            let value = arguments.next().expect("two arguments");
            let target = arguments.next().expect("two arguments");
            Ok(located(
                ExprKind::TypeAscription {
                    value: Box::new(value),
                    target: Box::new(target),
                },
                self.location(invocation),
            ))
        } else if name == "cast" {
            if arguments.len() != 2 {
                return Err(self.error(
                    invocation,
                    format!(
                        "cast! expects a value and a Type, found {} arguments",
                        arguments.len()
                    ),
                ));
            }
            let mut arguments = arguments.into_iter();
            let value = arguments.next().expect("two arguments");
            let target = arguments.next().expect("two arguments");
            Ok(located(
                ExprKind::CheckedCast {
                    value: Box::new(value),
                    target: Box::new(target),
                },
                self.location(invocation),
            ))
        } else if matches!(name, "should_ok" | "must_ok") {
            self.lower_check(name, arguments, invocation)
        } else if matches!(name, "try_unwrap" | "unwrap") {
            self.lower_unwrap(name, arguments, invocation)
        } else if name == "fail" {
            self.lower_fail(arguments, invocation)
        } else {
            if arguments.len() != 1 {
                return Err(self.error(
                    invocation,
                    format!(
                        "{name}! expects exactly one argument, found {}",
                        arguments.len()
                    ),
                ));
            }
            let argument = Box::new(arguments.into_iter().next().unwrap());
            let kind = ExprKind::Panic { message: argument };
            Ok(located(kind, self.location(invocation)))
        }
    }

    fn lower_debug(
        &self,
        arguments: &[NodeRef],
        receiver: Option<Expr>,
        node: NodeRef,
    ) -> Result<Expr, Diagnostic> {
        if !(1..=2).contains(&arguments.len()) {
            return Err(self.error(
                node,
                format!(
                    "dbg! expects an expression and an optional String literal, found {} arguments",
                    arguments.len()
                ),
            ));
        }
        let value_node = arguments[0];
        let value = match receiver {
            Some(receiver) => receiver,
            None => self.expression(value_node)?,
        };
        let message = if let Some(message_node) = arguments.get(1).copied() {
            match self.expression(message_node)?.value {
                ExprKind::String(message) => Some(message),
                _ => {
                    return Err(self.error(message_node, "dbg! message must be a String literal"));
                }
            }
        } else {
            None
        };
        Ok(located(
            ExprKind::Debug {
                value: Box::new(value),
                message,
                expression: self.text(value_node).into_owned(),
            },
            self.location(node),
        ))
    }

    fn lower_blame(
        &self,
        name: &str,
        arguments: Vec<Expr>,
        node: NodeRef,
    ) -> Result<Expr, Diagnostic> {
        if arguments.is_empty() {
            return Err(self.error(
                node,
                format!("{name}! expects a message followed by zero or more subjects"),
            ));
        }
        let location = self.location(node);
        let mut arguments = arguments.into_iter();
        let message = arguments.next().expect("blame message was checked");
        // Keep the explicit subject boundary in the internal envelope. The VM
        // uses this tuple to retain one ordered provenance location per subject.
        let data = located(ExprKind::Tuple(arguments.collect()), location);
        let rule = located(ExprKind::String(format!("{name}!")), location);
        let fields = [("data", data), ("message", message), ("rule", rule)]
            .into_iter()
            .map(|(name, value)| {
                located(
                    DictFieldKind {
                        decorators: Vec::new(),
                        name: Some(located(name.into(), location)),
                        value,
                    },
                    location,
                )
            })
            .collect();
        Ok(located(ExprKind::Dict(fields), location))
    }

    fn lower_fail(&self, arguments: Vec<Expr>, node: NodeRef) -> Result<Expr, Diagnostic> {
        if arguments.is_empty() {
            return Err(self.error(
                node,
                "fail! expects a message followed by zero or more subjects",
            ));
        }
        let location = self.location(node);
        let blame = self.lower_blame("fail", arguments, node)?;
        Ok(located(
            ExprKind::Raise {
                error: Box::new(blame),
            },
            location,
        ))
    }

    fn lower_check(
        &self,
        name: &str,
        arguments: Vec<Expr>,
        node: NodeRef,
    ) -> Result<Expr, Diagnostic> {
        if arguments.is_empty() {
            return Err(self.error(
                node,
                format!("{name}! expects a checker followed by zero or more arguments"),
            ));
        }
        let location = self.location(node);
        let prefix = format!("${name}:{}", location.range().start);
        let identifier = |suffix: &str| located(format!("{prefix}:{suffix}"), location);
        let variable = |suffix: &str| located(ExprKind::Variable(identifier(suffix)), location);
        let binding = |suffix: &str, value| {
            located(
                BindingData {
                    decorators: Vec::new(),
                    kind: BindingKind::Let,
                    declared_initializer: None,
                    imported_name: None,
                    name: identifier(suffix),
                    type_parameters: Vec::new(),
                    type_parameter_bounds: Vec::new(),
                    annotation: None,
                    value,
                },
                location,
            )
        };
        let mut arguments = arguments.into_iter();
        let checker = arguments.next().expect("check checker was checked");
        let values = arguments.collect::<Vec<_>>();
        let mut bindings = vec![binding("checker", checker)];
        bindings.extend(
            values
                .into_iter()
                .enumerate()
                .map(|(index, value)| binding(&format!("argument:{index}"), value)),
        );
        let call_arguments = (0..bindings.len() - 1)
            .map(|index| variable(&format!("argument:{index}")))
            .collect::<Vec<_>>();
        let evidence = located(ExprKind::Tuple(call_arguments.clone()), location);
        let call = located(
            ExprKind::Call {
                callee: Box::new(variable("checker")),
                arguments: call_arguments,
            },
            location,
        );
        let payload = identifier("payload");
        let message = identifier("message");
        let tagged_pattern = |tag: &str, payload: Identifier| {
            located(
                PatternKind::Tagged {
                    tag: tag.into(),
                    payload: Box::new(located(PatternKind::Binding(payload), location)),
                },
                location,
            )
        };
        let tagged_value = |tag: &str, value: Expr| {
            located(
                ExprKind::Call {
                    callee: Box::new(located(ExprKind::Atom(tag.into()), location)),
                    arguments: vec![value],
                },
                location,
            )
        };
        let diagnostic = located(
            ExprKind::Call {
                callee: Box::new(located(
                    ExprKind::Variable(located("\0telora_warn".into(), location)),
                    location,
                )),
                arguments: vec![
                    located(ExprKind::Variable(message.clone()), location),
                    evidence,
                ],
            },
            location,
        );
        let rejected = if name == "should_ok" {
            located(
                ExprKind::Block(located(
                    BlockKind {
                        bindings: vec![binding("warning", diagnostic)],
                        result: Box::new(located(ExprKind::Atom("None".into()), location)),
                    },
                    location,
                )),
                location,
            )
        } else {
            let message_value = located(ExprKind::Variable(message.clone()), location);
            let envelope = self.lower_blame(
                "must_ok",
                std::iter::once(message_value)
                    .chain(
                        (0..bindings.len() - 1).map(|index| variable(&format!("argument:{index}"))),
                    )
                    .collect(),
                node,
            )?;
            located(
                ExprKind::Raise {
                    error: Box::new(envelope),
                },
                location,
            )
        };
        let result = located(
            ExprKind::Match {
                value: Box::new(call),
                arms: vec![
                    located(
                        MatchArmKind {
                            pattern: tagged_pattern("Ok", payload.clone()),
                            guard: None,
                            value: if name == "should_ok" {
                                tagged_value("Some", located(ExprKind::Variable(payload), location))
                            } else {
                                located(ExprKind::Variable(payload), location)
                            },
                            irrefutable_required: false,
                        },
                        location,
                    ),
                    located(
                        MatchArmKind {
                            pattern: tagged_pattern("Err", message),
                            guard: None,
                            value: rejected,
                            irrefutable_required: false,
                        },
                        location,
                    ),
                ],
            },
            location,
        );
        Ok(located(
            ExprKind::Block(located(
                BlockKind {
                    bindings,
                    result: Box::new(result),
                },
                location,
            )),
            location,
        ))
    }

    fn lower_unwrap(
        &self,
        name: &str,
        arguments: Vec<Expr>,
        node: NodeRef,
    ) -> Result<Expr, Diagnostic> {
        if arguments.len() != 1 {
            return Err(self.error(
                node,
                format!(
                    "{name}! expects exactly one Result value, found {} arguments",
                    arguments.len()
                ),
            ));
        }
        let location = self.location(node);
        let prefix = format!("${name}:{}", location.range().start);
        let identifier = |suffix: &str| located(format!("{prefix}:{suffix}"), location);
        let variable = |suffix: &str| located(ExprKind::Variable(identifier(suffix)), location);
        let binding = |suffix: &str, value| {
            located(
                BindingData {
                    decorators: Vec::new(),
                    kind: BindingKind::Let,
                    declared_initializer: None,
                    imported_name: None,
                    name: identifier(suffix),
                    type_parameters: Vec::new(),
                    type_parameter_bounds: Vec::new(),
                    annotation: None,
                    value,
                },
                location,
            )
        };
        let result = arguments
            .into_iter()
            .next()
            .expect("unwrap arity was checked");
        let payload = identifier("payload");
        let message = identifier("message");
        let tagged_pattern = |tag: &str, payload: Identifier| {
            located(
                PatternKind::Tagged {
                    tag: tag.into(),
                    payload: Box::new(located(PatternKind::Binding(payload), location)),
                },
                location,
            )
        };
        let success = if name == "try_unwrap" {
            located(
                ExprKind::Call {
                    callee: Box::new(located(ExprKind::Atom("Some".into()), location)),
                    arguments: vec![located(ExprKind::Variable(payload.clone()), location)],
                },
                location,
            )
        } else {
            located(ExprKind::Variable(payload.clone()), location)
        };
        let rejected = if name == "try_unwrap" {
            let warning = located(
                ExprKind::Call {
                    callee: Box::new(located(
                        ExprKind::Variable(located("\0telora_warn".into(), location)),
                        location,
                    )),
                    arguments: vec![
                        located(ExprKind::Variable(message.clone()), location),
                        variable("result"),
                    ],
                },
                location,
            );
            located(
                ExprKind::Block(located(
                    BlockKind {
                        bindings: vec![binding("warning", warning)],
                        result: Box::new(located(ExprKind::Atom("None".into()), location)),
                    },
                    location,
                )),
                location,
            )
        } else {
            let envelope = self.lower_blame(
                "unwrap",
                vec![
                    located(ExprKind::Variable(message.clone()), location),
                    variable("result"),
                ],
                node,
            )?;
            located(
                ExprKind::Raise {
                    error: Box::new(envelope),
                },
                location,
            )
        };
        let matched = located(
            ExprKind::Match {
                value: Box::new(variable("result")),
                arms: vec![
                    located(
                        MatchArmKind {
                            pattern: tagged_pattern("Ok", payload),
                            guard: None,
                            value: success,
                            irrefutable_required: false,
                        },
                        location,
                    ),
                    located(
                        MatchArmKind {
                            pattern: tagged_pattern("Err", message),
                            guard: None,
                            value: rejected,
                            irrefutable_required: false,
                        },
                        location,
                    ),
                ],
            },
            location,
        );
        Ok(located(
            ExprKind::Block(located(
                BlockKind {
                    bindings: vec![binding("result", result)],
                    result: Box::new(matched),
                },
                location,
            )),
            location,
        ))
    }

    fn lower_interpreter(
        &self,
        node: NodeRef,
        type_parameters: &[Identifier],
        contract: Option<&Expr>,
    ) -> Result<Expr, Diagnostic> {
        let operands = self
            .children(node)
            .filter(|child| self.is_expression(*child))
            .collect::<Vec<_>>();
        if operands.len() != 1 {
            return Err(self.error(
                node,
                format!(
                    "interpreter! expects exactly one argument, found {}",
                    operands.len()
                ),
            ));
        }
        let operand = operands[0];
        let location = self.location(node);
        let operand = self.expression(operand)?;
        let plan = contract
            .and_then(|contract| interpreter_syntax_plan(type_parameters, contract))
            .unwrap_or_else(|| InterpreterSyntaxPlan {
                witness_count: 1,
                parameters: vec![Some(0), Some(0)],
            });
        let elaboration = interpreter_expansion(operand.clone(), location, &plan);
        Ok(located(
            ExprKind::Interpreter {
                operand: Box::new(operand),
                elaboration: Box::new(elaboration),
            },
            location,
        ))
    }

}
