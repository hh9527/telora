impl<'a> Lowerer<'a> {
    fn match_arm(&self, node: NodeRef) -> Result<MatchArm, Diagnostic> {
        let arrow = self.first_token(node, Token::FatArrow)?;
        let arrow_start = self.cst.span(arrow).start;
        let pattern = self
            .children(node)
            .find(|child| self.is_pattern(*child) && self.cst.span(*child).end <= arrow_start)
            .ok_or_else(|| self.error(node, "match arm has no pattern"))?;
        let value = self
            .children(node)
            .find(|child| self.is_expression(*child) && self.cst.span(*child).start > arrow_start)
            .ok_or_else(|| self.error(node, "match arm has no value"))?;
        let guard = self
            .token_children(node, Token::If)
            .next()
            .and_then(|if_token| {
                self.children(node).find(|child| {
                    self.is_expression(*child)
                        && self.cst.span(*child).start > self.cst.span(if_token).end
                        && self.cst.span(*child).end <= arrow_start
                })
            })
            .map(|guard| self.expression(guard))
            .transpose()?;
        Ok(located(
            MatchArmKind {
                pattern: self.pattern(pattern)?,
                guard,
                value: self.expression(value)?,
                irrefutable_required: false,
            },
            self.location(node),
        ))
    }

    fn pattern(&self, node: NodeRef) -> Result<Pattern, Diagnostic> {
        if let Node::Token(token, _) = self.cst.get(node) {
            let inner = match token {
                Token::Identifier | Token::Placeholder => {
                    let name = self.text(node);
                    if name == "_" {
                        PatternKind::Wildcard
                    } else {
                        PatternKind::Binding(self.identifier(node))
                    }
                }
                Token::Int => PatternKind::Int(
                    self.text(node)
                        .parse()
                        .map_err(|_| self.error(node, "invalid Int pattern"))?,
                ),
                Token::Float => PatternKind::Float(
                    parse_float_literal(&self.text(node))
                        .map_err(|message| self.error(node, message))?,
                ),
                Token::Atom => {
                    PatternKind::Atom(self.text(node).trim_start_matches('\'').to_owned())
                }
                _ => return Err(self.error(node, "expected pattern token")),
            };
            return Ok(located(inner, self.location(node)));
        }
        let rule = self
            .rule(node)
            .ok_or_else(|| self.error(node, "expected pattern"))?;
        if rule == Rule::Pattern {
            return self.pattern(
                self.first_rule(node)
                    .ok_or_else(|| self.error(node, "empty pattern"))?,
            );
        }
        let inner = match rule {
            Rule::IdentifierPattern => {
                if self
                    .token_children(node, Token::Placeholder)
                    .next()
                    .is_some()
                {
                    PatternKind::Wildcard
                } else {
                    PatternKind::Binding(
                        self.identifier(self.first_token(node, Token::Identifier)?),
                    )
                }
            }
            Rule::IntPattern => PatternKind::Int(
                self.text(self.first_token(node, Token::Int)?)
                    .parse()
                    .map_err(|_| self.error(node, "invalid Int pattern"))?,
            ),
            Rule::FloatPattern => {
                let token = self.first_token(node, Token::Float)?;
                PatternKind::Float(
                    parse_float_literal(&self.text(token))
                        .map_err(|message| self.error(token, message))?,
                )
            }
            Rule::StringPattern => {
                let string = self
                    .rule_children(node)
                    .find(|child| self.rule(*child) == Some(Rule::StringLiteral))
                    .ok_or_else(|| self.error(node, "string pattern has no literal"))?;
                PatternKind::String(self.plain_string(string, "string pattern")?)
            }
            Rule::AtomPattern => PatternKind::Atom(
                self.text(self.first_token(node, Token::Atom)?)
                    .trim_start_matches('\'')
                    .to_owned(),
            ),
            Rule::TaggedPattern => PatternKind::Tagged {
                tag: self
                    .text(self.first_token(node, Token::Atom)?)
                    .trim_start_matches('\'')
                    .to_owned(),
                payload: Box::new(
                    self.pattern(
                        self.children(node)
                            .filter(|child| {
                                !matches!(self.cst.get(*child), Node::Token(Token::Atom, _))
                            })
                            .find(|child| self.is_pattern(*child))
                            .ok_or_else(|| self.error(node, "tagged pattern has no payload"))?,
                    )?,
                ),
            },
            Rule::TuplePattern => PatternKind::Tuple(
                self.children(node)
                    .filter(|child| self.is_pattern(*child))
                    .map(|child| self.pattern(child))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            Rule::StructPattern => PatternKind::Struct(
                self.rule_children(node)
                    .filter(|child| self.rule(*child) == Some(Rule::StructPatternField))
                    .map(|field| self.struct_pattern_field(field))
                    .collect::<Result<Vec<_>, _>>()?,
            ),
            _ => return Err(self.error(node, "unexpected pattern rule")),
        };
        Ok(located(inner, self.location(node)))
    }

    fn struct_pattern_field(&self, node: NodeRef) -> Result<StructPatternField, Diagnostic> {
        let name_node = self.first_token(node, Token::Identifier)?;
        let name = self.identifier(name_node);
        let pattern = self
            .children(node)
            .find(|child| *child != name_node && self.is_pattern(*child))
            .map(|pattern| self.pattern(pattern))
            .transpose()?
            .unwrap_or_else(|| located(PatternKind::Binding(name.clone()), name.location));
        Ok(StructPatternField { name, pattern })
    }

    fn parameters(&self, node: NodeRef) -> Result<Vec<ClosureParameter>, Diagnostic> {
        self.children(node)
            .filter(|child| self.rule(*child) == Some(Rule::Parameter))
            .map(|parameter| {
                let name = self
                    .token_children(parameter, Token::Identifier)
                    .next()
                    .map(|name| self.identifier(name))
                    .ok_or_else(|| self.error(parameter, "closure parameter has no name"))?;
                let annotation = self
                    .token_children(parameter, Token::Colon)
                    .next()
                    .and_then(|colon| {
                        self.children(parameter).find(|child| {
                            self.is_expression(*child)
                                && self.cst.span(*child).start > self.cst.span(colon).start
                        })
                    })
                    .map(|annotation| self.expression(annotation))
                    .transpose()?;
                Ok(ClosureParameter { name, annotation })
            })
            .collect()
    }

    fn function_contract_expression(
        &self,
        parameters: Vec<Expr>,
        result: Expr,
        location: Location,
    ) -> Expr {
        let parameters = located(ExprKind::Array(parameters), location);
        let callee_name = located("Func".to_owned(), location);
        located(
            ExprKind::Call {
                callee: Box::new(located(ExprKind::Variable(callee_name), location)),
                arguments: vec![parameters, result],
            },
            location,
        )
    }

    fn contract_expression(&self, node: NodeRef) -> Result<Expr, Diagnostic> {
        let location = self.location(node);
        match self.rule(node) {
            Some(Rule::Contract) => {
                let inner = self
                    .first_rule(node)
                    .ok_or_else(|| self.error(node, "empty contract"))?;
                self.contract_expression(inner)
            }
            Some(Rule::ContractExpr) => {
                let path = self
                    .rule_children(node)
                    .find(|child| self.rule(*child) == Some(Rule::ContractPath))
                    .unwrap_or(node);
                let mut names = self
                    .token_children(path, Token::Identifier)
                    .map(|token| self.identifier(token));
                let name = names
                    .next()
                    .ok_or_else(|| self.error(node, "contract has no name"))?;
                let mut callee = located(ExprKind::Variable(name), location);
                for field in names {
                    callee = located(
                        ExprKind::Field {
                            receiver: Box::new(callee),
                            field,
                        },
                        location,
                    );
                }
                let arguments = self
                    .rule_children(node)
                    .filter(|child| {
                        matches!(
                            self.rule(*child),
                            Some(
                                Rule::Contract
                                    | Rule::ContractExpr
                                    | Rule::FunctionContract
                                    | Rule::ContractArray
                            )
                        )
                    })
                    .map(|child| self.contract_expression(child))
                    .collect::<Result<Vec<_>, _>>()?;
                if arguments.is_empty() {
                    Ok(callee)
                } else {
                    Ok(located(
                        ExprKind::Call {
                            callee: Box::new(callee),
                            arguments,
                        },
                        location,
                    ))
                }
            }
            Some(Rule::ContractArray) => Ok(located(
                ExprKind::Array(
                    self.rule_children(node)
                        .filter(|child| {
                            matches!(
                                self.rule(*child),
                                Some(Rule::Contract | Rule::ContractExpr | Rule::FunctionContract)
                            )
                        })
                        .map(|child| self.contract_expression(child))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                location,
            )),
            Some(Rule::FunctionContract) => {
                let mut parts = self
                    .rule_children(node)
                    .filter(|child| {
                        matches!(
                            self.rule(*child),
                            Some(Rule::Contract | Rule::ContractExpr | Rule::FunctionContract)
                        )
                    })
                    .map(|child| self.contract_expression(child))
                    .collect::<Result<Vec<_>, _>>()?;
                let result = parts
                    .pop()
                    .ok_or_else(|| self.error(node, "function contract has no result"))?;
                Ok(self.function_contract_expression(parts, result, location))
            }
            _ => Err(self.error(node, "invalid contract")),
        }
    }

    fn decorators(&self, node: NodeRef) -> Result<Vec<Decorator>, Diagnostic> {
        self.rule_children(node)
            .filter(|child| self.rule(*child) == Some(Rule::Decorator))
            .map(|decorator| {
                let path = self
                    .rule_children(decorator)
                    .find(|child| self.rule(*child) == Some(Rule::DecoratorPath))
                    .ok_or_else(|| self.error(decorator, "decorator has no path"))?;
                let mut identifiers = self
                    .token_children(path, Token::Identifier)
                    .map(|token| self.identifier(token));
                let first = identifiers
                    .next()
                    .ok_or_else(|| self.error(path, "decorator path is empty"))?;
                let mut callee = located(ExprKind::Variable(first.clone()), first.location);
                for field in identifiers {
                    let location = Location::new(
                        callee.location.source,
                        crate::source::TextRange::from_usize(
                            callee.location.start as usize..field.location.end as usize,
                        )
                        .expect("decorator path is within a parsed source"),
                    );
                    callee = located(
                        ExprKind::Field {
                            receiver: Box::new(callee),
                            field,
                        },
                        location,
                    );
                }
                let arguments_node = self
                    .rule_children(decorator)
                    .find(|child| self.rule(*child) == Some(Rule::Arguments));
                let arguments = arguments_node
                    .map(|arguments| self.expression_children(arguments))
                    .transpose()?
                    .unwrap_or_default();
                Ok(located(
                    DecoratorKind {
                        callee,
                        arguments,
                        configured: arguments_node.is_some(),
                    },
                    self.location(decorator),
                ))
            })
            .collect()
    }

    fn declared_type_initializer(&self, node: NodeRef) -> Result<(Expr, Decorator), Diagnostic> {
        let (operation, members) = match self.rule(node) {
            Some(Rule::StructInitializer) => {
                let mut fields = Vec::new();
                let mut names = std::collections::HashSet::new();
                for field in self
                    .rule_children(node)
                    .filter(|child| self.rule(*child) == Some(Rule::StructInitializerField))
                {
                    let name_node = self.first_token(field, Token::Identifier)?;
                    let name = self.identifier(name_node);
                    if !names.insert(name.value.clone()) {
                        return Err(self.error(
                            name_node,
                            format!("duplicate Struct field {:?}", name.value),
                        ));
                    }
                    let colon = self.first_token(field, Token::Colon)?;
                    let value_node = self
                        .children(field)
                        .find(|child| {
                            self.is_expression(*child)
                                && self.cst.span(*child).start > self.cst.span(colon).start
                        })
                        .ok_or_else(|| self.error(field, "Struct field has no type"))?;
                    let decorators = self.decorators(field)?;
                    let value = self.expression(value_node)?;
                    fields.push(located(
                        DictFieldKind {
                            decorators,
                            name: Some(name),
                            value,
                        },
                        self.location(field),
                    ));
                }
                ("\0telora_struct", fields)
            }
            Some(Rule::EnumInitializer) => {
                let mut variants = Vec::new();
                let mut names = std::collections::HashSet::new();
                for variant in self
                    .rule_children(node)
                    .filter(|child| self.rule(*child) == Some(Rule::EnumInitializerVariant))
                {
                    let tag_node = self.first_token(variant, Token::Atom)?;
                    let name = located(
                        self.text(tag_node).trim_start_matches('\'').to_owned(),
                        self.location(tag_node),
                    );
                    if !names.insert(name.value.clone()) {
                        return Err(self
                            .error(tag_node, format!("duplicate Enum variant {:?}", name.value)));
                    }
                    let payload = if let Ok(left_paren) = self.first_token(variant, Token::LParen) {
                        let payload = self
                            .children(variant)
                            .find(|child| {
                                self.is_expression(*child)
                                    && self.cst.span(*child).start > self.cst.span(left_paren).start
                            })
                            .ok_or_else(|| {
                                self.error(variant, "Enum variant has no payload type")
                            })?;
                        self.expression(payload)?
                    } else {
                        located(ExprKind::Atom("None".to_owned()), self.location(tag_node))
                    };
                    let decorators = self.decorators(variant)?;
                    let value = payload;
                    variants.push(located(
                        DictFieldKind {
                            decorators,
                            name: Some(name),
                            value,
                        },
                        self.location(variant),
                    ));
                }
                ("\0telora_enum", variants)
            }
            _ => return Err(self.error(node, "invalid type initializer")),
        };
        let location = self.location(node);
        let callee_token = self
            .children(node)
            .find(|child| {
                matches!(
                    self.cst.get(*child),
                    Node::Token(Token::StructInitializer | Token::EnumInitializer, _)
                )
            })
            .unwrap_or(node);
        let operation_location = self.location(callee_token);
        Ok((
            located(ExprKind::Dict(members), location),
            located(
                DecoratorKind {
                    callee: located(
                        ExprKind::Variable(located(operation.to_owned(), operation_location)),
                        operation_location,
                    ),
                    arguments: Vec::new(),
                    configured: false,
                },
                operation_location,
            ),
        ))
    }

    fn apply_decorators(
        &self,
        decorators: &[Decorator],
        kind: &str,
        name: &Identifier,
        mut value: Expr,
        target_location: Location,
    ) -> Expr {
        let context = self.decorator_context(kind, name, target_location);
        for decorator in decorators.iter().rev() {
            let callee = if decorator.value.configured {
                located(
                    ExprKind::Call {
                        callee: Box::new(decorator.value.callee.clone()),
                        arguments: decorator.value.arguments.clone(),
                    },
                    decorator.location,
                )
            } else {
                decorator.value.callee.clone()
            };
            value = located(
                ExprKind::Call {
                    callee: Box::new(callee),
                    arguments: vec![context.clone(), value],
                },
                decorator.location,
            );
        }
        value
    }

    fn decorator_context(&self, kind: &str, name: &Identifier, target_location: Location) -> Expr {
        located(
            ExprKind::Dict(vec![
                located(
                    DictFieldKind {
                        decorators: Vec::new(),
                        name: Some(located("kind".to_owned(), target_location)),
                        value: located(ExprKind::Atom(kind.to_owned()), target_location),
                    },
                    target_location,
                ),
                located(
                    DictFieldKind {
                        decorators: Vec::new(),
                        name: Some(located("name".to_owned(), name.location)),
                        value: located(ExprKind::String(name.value.clone()), name.location),
                    },
                    name.location,
                ),
            ]),
            target_location,
        )
    }

}
