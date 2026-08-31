impl<'a> Lowerer<'a> {
    fn new(
        source_id: SourceId,
        source: &'a crate::document::DocumentText,
        cst: &'a CstData,
    ) -> Self {
        Self {
            source_id,
            source,
            cst,
        }
    }

    fn program(&self) -> Result<Program, Diagnostic> {
        let root = NodeRef::ROOT;
        let body_node = self
            .rule_children(root)
            .find(|node| self.rule(*node) == Some(Rule::Body))
            .or_else(|| self.first_rule(root))
            .ok_or_else(|| self.error(root, "program has no body"))?;
        let authored_result = self
            .children(body_node)
            .any(|child| self.is_expression(child));
        let mut body = self.block_body_with_destructuring(body_node, false)?;
        let exports = body
            .value
            .bindings
            .iter()
            .filter(|binding| binding.value.kind == BindingKind::Export)
            .collect::<Vec<_>>();
        if authored_result && !exports.is_empty() {
            return Err(self.error(
                body_node,
                "top-level expressions are not supported; bind the computation with def and export the intended result",
            ));
        }
        let mut public_names = HashMap::new();
        for export in exports {
            if let Some(first) =
                public_names.insert(export.value.name.value.clone(), export.value.name.location)
            {
                return Err(Diagnostic::error(
                    format!("duplicate export {:?}", export.value.name.value),
                    export.value.name.location,
                )
                .with_secondary("first exported here", first));
            }
        }
        if !authored_result {
            body.value.result = Box::new(synthesize_export_record(
                &body.value.bindings,
                self.location(body_node),
            ));
        }
        Ok(located(
            ProgramKind {
                body,
                authored_result,
            },
            self.location(root),
        ))
    }

    fn recover_program(&self, diagnostics: &mut Vec<Diagnostic>) -> RecoveredProgram {
        use crate::syntax::telora::ast::{AstNode, Program as SyntaxProgram};

        let root = SyntaxProgram::root(self.cst);
        let mut bindings = Vec::new();
        let mut result = None;
        if let Some(body) = root.body() {
            for binding in body.bindings() {
                let node = binding.syntax().node_ref();
                let lowered = match self.rule(node) {
                    Some(Rule::ImportBinding) => self.import_bindings(node),
                    Some(Rule::ExportStatement) => self.export_bindings(node),
                    _ => self.binding(node).map(|binding| vec![binding]),
                };
                match lowered {
                    Ok(lowered) => bindings.extend(lowered),
                    Err(diagnostic) => push_unique_diagnostic(diagnostics, diagnostic),
                }
            }
            if let Some(expression) = body.result() {
                match self.expression(expression.syntax().node_ref()) {
                    Ok(expression) => result = Some(expression),
                    Err(diagnostic) => push_unique_diagnostic(diagnostics, diagnostic),
                }
            }
        }
        diagnostics.sort_by_key(|diagnostic| {
            diagnostic
                .labels
                .first()
                .map_or(0, |label| label.location.start)
        });
        if result.is_none()
            && bindings
                .iter()
                .any(|binding| binding.value.kind == BindingKind::Export)
        {
            result = Some(synthesize_export_record(
                &bindings,
                self.location(NodeRef::ROOT),
            ));
        }
        RecoveredProgram {
            location: self.location(NodeRef::ROOT),
            bindings,
            result,
        }
    }

    fn block_body(&self, node: NodeRef) -> Result<Block, Diagnostic> {
        self.block_body_with_destructuring(node, true)
    }

    fn block_body_with_destructuring(
        &self,
        node: NodeRef,
        allow_destructuring: bool,
    ) -> Result<Block, Diagnostic> {
        let body = if self.rule(node) == Some(Rule::Block) {
            self.rule_children(node)
                .find(|child| self.rule(*child) == Some(Rule::Body))
                .ok_or_else(|| self.error(node, "block has no body"))?
        } else {
            node
        };
        let children = self.children(body).collect::<Vec<_>>();
        let mut entries = Vec::new();
        let mut result = None;
        for child in children {
            match self.rule(child) {
                Some(
                    Rule::LetBinding
                    | Rule::DeclBinding
                    | Rule::DefBinding
                    | Rule::NativeBinding
                    | Rule::NativeTypeBinding
                    | Rule::TypeBinding,
                ) => entries.push(BlockEntry::Binding(self.binding(child)?)),
                Some(Rule::TraitBinding | Rule::ImplBinding) => {
                    if allow_destructuring {
                        return Err(self.error(
                            child,
                            "trait and impl declarations are allowed only at module top level",
                        ));
                    }
                    entries.push(BlockEntry::Binding(self.binding(child)?));
                }
                Some(Rule::ImportBinding) => entries.extend(
                    self.import_bindings(child)?
                        .into_iter()
                        .map(BlockEntry::Binding),
                ),
                Some(Rule::ExportStatement) => entries.extend(if allow_destructuring {
                    return Err(self.error(
                        child,
                        "export declarations are allowed only at module top level",
                    ));
                } else {
                    self.export_bindings(child)?
                        .into_iter()
                        .map(BlockEntry::Binding)
                        .collect::<Vec<_>>()
                }),
                Some(Rule::LetPatternBinding) => {
                    if !allow_destructuring {
                        return Err(self.error(
                            child,
                            "destructuring let is allowed only inside a local block",
                        ));
                    }
                    let (pattern, value) = self.let_pattern_binding(child)?;
                    entries.push(BlockEntry::Destructure {
                        pattern,
                        value,
                        location: self.location(child),
                    });
                }
                Some(Rule::LetElseBinding) => {
                    if !allow_destructuring {
                        return Err(
                            self.error(child, "let else is allowed only inside a local block")
                        );
                    }
                    let (pattern, value, else_branch) = self.let_else_binding(child)?;
                    entries.push(BlockEntry::LetElse {
                        pattern,
                        value,
                        else_branch,
                        location: self.location(child),
                    });
                }
                Some(Rule::Binding) => {
                    let inner = self
                        .first_rule(child)
                        .ok_or_else(|| self.error(child, "empty binding"))?;
                    if self.rule(inner) == Some(Rule::LetElseBinding) {
                        if !allow_destructuring {
                            return Err(
                                self.error(inner, "let else is allowed only inside a local block")
                            );
                        }
                        let (pattern, value, else_branch) = self.let_else_binding(inner)?;
                        entries.push(BlockEntry::LetElse {
                            pattern,
                            value,
                            else_branch,
                            location: self.location(inner),
                        });
                    } else if self.rule(inner) == Some(Rule::LetPatternBinding) {
                        if !allow_destructuring {
                            return Err(self.error(
                                inner,
                                "destructuring let is allowed only inside a local block",
                            ));
                        }
                        let (pattern, value) = self.let_pattern_binding(inner)?;
                        entries.push(BlockEntry::Destructure {
                            pattern,
                            value,
                            location: self.location(inner),
                        });
                    } else if self.rule(inner) == Some(Rule::ImportBinding) {
                        entries.extend(
                            self.import_bindings(inner)?
                                .into_iter()
                                .map(BlockEntry::Binding),
                        );
                    } else if self.rule(inner) == Some(Rule::ExportStatement) {
                        if allow_destructuring {
                            return Err(self.error(
                                inner,
                                "export declarations are allowed only at module top level",
                            ));
                        }
                        entries.extend(
                            self.export_bindings(inner)?
                                .into_iter()
                                .map(BlockEntry::Binding),
                        );
                    } else {
                        entries.push(BlockEntry::Binding(self.binding(inner)?));
                    }
                }
                Some(_) => result = Some(self.expression(child)?),
                None if self.is_expression(child) => result = Some(self.expression(child)?),
                None => {}
            }
        }
        let result = if let Some(result) = result {
            result
        } else if allow_destructuring {
            return Err(self.error(body, "a block requires a result expression"));
        } else {
            located(ExprKind::Tuple(Vec::new()), self.location(body))
        };
        let block_location = self.location(node);
        let mut block = located(
            BlockKind {
                bindings: Vec::new(),
                result: Box::new(result),
            },
            block_location,
        );
        for entry in entries.into_iter().rev() {
            match entry {
                BlockEntry::Binding(binding) => block.value.bindings.insert(0, binding),
                BlockEntry::Destructure {
                    pattern,
                    value,
                    location,
                } => {
                    let body_location = block.location;
                    let body = located(ExprKind::Block(block), body_location);
                    let arm = located(
                        MatchArmKind {
                            pattern,
                            guard: None,
                            value: body,
                            irrefutable_required: true,
                        },
                        location,
                    );
                    block = located(
                        BlockKind {
                            bindings: Vec::new(),
                            result: Box::new(located(
                                ExprKind::Match {
                                    value: Box::new(value),
                                    arms: vec![arm],
                                },
                                location,
                            )),
                        },
                        block_location,
                    );
                }
                BlockEntry::LetElse {
                    pattern,
                    value,
                    else_branch,
                    location,
                } => {
                    block = located(
                        BlockKind {
                            bindings: Vec::new(),
                            result: Box::new(located(
                                ExprKind::LetElse {
                                    pattern,
                                    value: Box::new(value),
                                    else_branch,
                                    body: block,
                                },
                                location,
                            )),
                        },
                        block_location,
                    );
                }
            }
        }
        Ok(block)
    }

    fn let_pattern_binding(&self, node: NodeRef) -> Result<(Pattern, Expr), Diagnostic> {
        let equal = self.first_token(node, Token::Equal)?;
        let equal_start = self.cst.span(equal).start;
        let pattern = self
            .children(node)
            .find(|child| self.is_pattern(*child) && self.cst.span(*child).end <= equal_start)
            .ok_or_else(|| self.error(node, "destructuring let has no pattern"))?;
        let value = self
            .children(node)
            .find(|child| self.is_expression(*child) && self.cst.span(*child).start > equal_start)
            .ok_or_else(|| self.error(node, "destructuring let has no value"))?;
        Ok((self.pattern(pattern)?, self.expression(value)?))
    }

    fn let_else_binding(&self, node: NodeRef) -> Result<(Pattern, Expr, Block), Diagnostic> {
        let equal = self.first_token(node, Token::Equal)?;
        let else_token = self.first_token(node, Token::Else)?;
        let pattern = self
            .children(node)
            .find(|child| {
                self.is_pattern(*child) && self.cst.span(*child).end <= self.cst.span(equal).start
            })
            .ok_or_else(|| self.error(node, "let else has no pattern"))?;
        let value = self
            .children(node)
            .find(|child| {
                self.is_expression(*child)
                    && self.cst.span(*child).start > self.cst.span(equal).end
                    && self.cst.span(*child).end <= self.cst.span(else_token).start
            })
            .ok_or_else(|| self.error(node, "let else has no value"))?;
        let else_branch = self
            .rule_children(node)
            .find(|child| self.rule(*child) == Some(Rule::Block))
            .ok_or_else(|| self.error(node, "let else has no else block"))?;
        Ok((
            self.pattern(pattern)?,
            self.expression(value)?,
            self.block_body(else_branch)?,
        ))
    }

    fn type_parameters(
        &self,
        node: NodeRef,
    ) -> Result<(Vec<Identifier>, Vec<Vec<Expr>>), Diagnostic> {
        let mut parameters = Vec::new();
        let mut bounds = Vec::new();
        for parameter in self
            .rule_children(node)
            .filter(|child| self.rule(*child) == Some(Rule::TypeParameter))
        {
            let name_node = self.first_token(parameter, Token::Identifier)?;
            let name = self.identifier(name_node);
            let parameter_bounds = self
                .rule_children(parameter)
                .filter(|child| self.rule(*child) == Some(Rule::TraitBound))
                .map(|bound| {
                    let contract = self
                        .rule_children(bound)
                        .find(|child| {
                            matches!(
                                self.rule(*child),
                                Some(Rule::Contract | Rule::ContractExpr | Rule::FunctionContract)
                            )
                        })
                        .ok_or_else(|| self.error(bound, "trait bound has no contract"))?;
                    self.contract_expression(contract)
                })
                .collect::<Result<Vec<_>, _>>()?;
            parameters.push(name);
            bounds.push(parameter_bounds);
        }
        Ok((parameters, bounds))
    }

    fn binding(&self, node: NodeRef) -> Result<Binding, Diagnostic> {
        let identifiers = self
            .token_children(node, Token::Identifier)
            .collect::<Vec<_>>();
        let name = if self.rule(node) == Some(Rule::ImplBinding) {
            located(
                format!("\0trait_impl_{}", self.cst.span(node).start),
                self.location(node),
            )
        } else {
            let name_node = identifiers
                .first()
                .copied()
                .ok_or_else(|| self.error(node, "binding has no name"))?;
            self.identifier(name_node)
        };
        match self
            .rule(node)
            .ok_or_else(|| self.error(node, "invalid binding"))?
        {
            Rule::LetBinding => {
                let equal = self.first_token(node, Token::Equal)?;
                let equal_start = self.cst.span(equal).start;
                let value_node = self
                    .children(node)
                    .find(|child| {
                        self.is_expression(*child) && self.cst.span(*child).start > equal_start
                    })
                    .ok_or_else(|| self.error(node, "binding has no value"))?;
                let annotation = if let Some(colon) = self.token_children(node, Token::Colon).next()
                {
                    let colon_start = self.cst.span(colon).start;
                    self.children(node)
                        .find(|child| {
                            self.is_expression(*child)
                                && self.cst.span(*child).start > colon_start
                                && self.cst.span(*child).end <= equal_start
                        })
                        .map(|child| self.expression(child))
                        .transpose()?
                } else {
                    None
                };
                let value = self.expression(value_node)?;
                Ok(located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::Let,
                        declared_initializer: None,
                        imported_name: None,
                        name,
                        type_parameters: Vec::new(),
                        type_parameter_bounds: Vec::new(),
                        annotation,
                        value,
                    },
                    self.location(node),
                ))
            }
            Rule::DeclBinding => {
                let scheme = self
                    .rule_children(node)
                    .find(|child| self.rule(*child) == Some(Rule::TypeScheme))
                    .ok_or_else(|| self.error(node, "declaration has no type scheme"))?;
                let (type_parameters, type_parameter_bounds) = self
                    .rule_children(scheme)
                    .find(|child| self.rule(*child) == Some(Rule::TypeParameters))
                    .map(|parameters| self.type_parameters(parameters))
                    .transpose()?
                    .unwrap_or_default();
                let contract_node = self
                    .rule_children(scheme)
                    .find(|child| {
                        matches!(
                            self.rule(*child),
                            Some(Rule::Contract | Rule::ContractExpr | Rule::FunctionContract)
                        )
                    })
                    .ok_or_else(|| self.error(node, "declaration has no contract"))?;
                let contract = self.contract_expression(contract_node)?;
                Ok(located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::Decl,
                        declared_initializer: None,
                        imported_name: None,
                        name,
                        type_parameters,
                        type_parameter_bounds,
                        annotation: Some(contract.clone()),
                        value: contract,
                    },
                    self.location(node),
                ))
            }
            Rule::NativeBinding => {
                let scheme = self
                    .rule_children(node)
                    .find(|child| self.rule(*child) == Some(Rule::TypeScheme))
                    .ok_or_else(|| self.error(node, "native declaration has no type scheme"))?;
                let (type_parameters, type_parameter_bounds) = self
                    .rule_children(scheme)
                    .find(|child| self.rule(*child) == Some(Rule::TypeParameters))
                    .map(|parameters| self.type_parameters(parameters))
                    .transpose()?
                    .unwrap_or_default();
                let contract_node = self
                    .rule_children(scheme)
                    .find(|child| {
                        matches!(
                            self.rule(*child),
                            Some(Rule::Contract | Rule::ContractExpr | Rule::FunctionContract)
                        )
                    })
                    .ok_or_else(|| self.error(node, "native declaration has no contract"))?;
                let contract = self.contract_expression(contract_node)?;
                Ok(located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::Native,
                        declared_initializer: None,
                        imported_name: None,
                        name,
                        type_parameters,
                        type_parameter_bounds,
                        annotation: Some(contract.clone()),
                        value: contract,
                    },
                    self.location(node),
                ))
            }
            Rule::NativeTypeBinding => {
                let slot = self.first_token(node, Token::Int)?;
                Ok(located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::NativeType,
                        declared_initializer: None,
                        imported_name: None,
                        name,
                        type_parameters: Vec::new(),
                        type_parameter_bounds: Vec::new(),
                        annotation: None,
                        value: located(
                            ExprKind::Int(self.text(slot).parse().map_err(|_| {
                                self.error(slot, "native type slot is outside the i64 range")
                            })?),
                            self.location(slot),
                        ),
                    },
                    self.location(node),
                ))
            }
            Rule::DefBinding => {
                let equal = self.first_token(node, Token::Equal)?;
                let scheme = self
                    .rule_children(node)
                    .find(|child| self.rule(*child) == Some(Rule::TypeScheme));
                let (type_parameters, type_parameter_bounds) = scheme
                    .and_then(|scheme| {
                        self.rule_children(scheme)
                            .find(|child| self.rule(*child) == Some(Rule::TypeParameters))
                    })
                    .map(|parameters| self.type_parameters(parameters))
                    .transpose()?
                    .unwrap_or_default();
                let annotation = scheme
                    .map(|scheme| {
                        self.rule_children(scheme)
                            .find(|child| {
                                matches!(
                                    self.rule(*child),
                                    Some(
                                        Rule::Contract
                                            | Rule::ContractExpr
                                            | Rule::FunctionContract
                                    )
                                )
                            })
                            .ok_or_else(|| self.error(node, "definition has no contract"))
                            .and_then(|contract| self.contract_expression(contract))
                    })
                    .transpose()?;
                let value_node = self
                    .children(node)
                    .find(|child| {
                        self.is_expression(*child)
                            && self.cst.span(*child).start > self.cst.span(equal).start
                    })
                    .ok_or_else(|| self.error(node, "definition has no value"))?;
                let interpreter_node = self.expression_head(value_node);
                let value = if matches!(
                    self.rule(interpreter_node),
                    Some(Rule::InterpreterIntrinsic | Rule::NamedIntrinsic)
                ) {
                    self.lower_contextual_intrinsic(
                        interpreter_node,
                        &type_parameters,
                        annotation.as_ref(),
                    )?
                } else {
                    self.expression(value_node)?
                };
                Ok(located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::Def,
                        declared_initializer: None,
                        imported_name: None,
                        name,
                        type_parameters,
                        type_parameter_bounds,
                        annotation,
                        value,
                    },
                    self.location(node),
                ))
            }
            Rule::TypeBinding => {
                let decorators = self.decorators(node)?;
                let (type_parameters, type_parameter_bounds) = self
                    .rule_children(node)
                    .find(|child| self.rule(*child) == Some(Rule::TypeParameters))
                    .map(|parameters| self.type_parameters(parameters))
                    .transpose()?
                    .unwrap_or_default();
                let equal = self.first_token(node, Token::Equal)?;
                let start = self.cst.span(equal).start;
                let initializer = self.children(node).find(|child| {
                    matches!(
                        self.rule(*child),
                        Some(Rule::StructInitializer | Rule::EnumInitializer)
                    ) && self.cst.span(*child).start > start
                });
                let declared_initializer =
                    initializer.and_then(|initializer| match self.rule(initializer) {
                        Some(Rule::StructInitializer) => Some(DeclaredInitializerKind::Struct),
                        Some(Rule::EnumInitializer) => Some(DeclaredInitializerKind::Enum),
                        _ => None,
                    });
                let value = if let Some(initializer) = initializer {
                    let (value, model) = self.declared_type_initializer(initializer)?;
                    self.apply_decorators(
                        std::slice::from_ref(&model),
                        "Type",
                        &name,
                        value,
                        self.location(node),
                    )
                } else {
                    self.expression(
                        self.children(node)
                            .find(|child| {
                                self.is_expression(*child) && self.cst.span(*child).start > start
                            })
                            .ok_or_else(|| self.error(node, "type has no value"))?,
                    )?
                };
                Ok(located(
                    BindingData {
                        decorators,
                        kind: BindingKind::Type,
                        declared_initializer,
                        imported_name: None,
                        name,
                        type_parameters,
                        type_parameter_bounds,
                        annotation: None,
                        value,
                    },
                    self.location(node),
                ))
            }
            Rule::TraitBinding => {
                let mut fields = Vec::new();
                let mut names = std::collections::HashSet::new();
                for member in self
                    .rule_children(node)
                    .filter(|child| self.rule(*child) == Some(Rule::TraitMember))
                {
                    let name_node = self.first_token(member, Token::Identifier)?;
                    let member_name = self.identifier(name_node);
                    if !names.insert(member_name.value.clone()) {
                        return Err(self.error(
                            name_node,
                            format!("duplicate trait member {:?}", member_name.value),
                        ));
                    }
                    let contract = self
                        .rule_children(member)
                        .find(|child| {
                            matches!(
                                self.rule(*child),
                                Some(Rule::Contract | Rule::ContractExpr | Rule::FunctionContract)
                            )
                        })
                        .ok_or_else(|| self.error(member, "trait member has no contract"))?;
                    fields.push(located(
                        DictFieldKind {
                            decorators: Vec::new(),
                            name: Some(member_name),
                            value: self.contract_expression(contract)?,
                        },
                        self.location(member),
                    ));
                }
                let operation_location = self.location(self.first_token(node, Token::Trait)?);
                let model = located(
                    DecoratorKind {
                        callee: located(
                            ExprKind::Variable(located(
                                "\0telora_struct".to_owned(),
                                operation_location,
                            )),
                            operation_location,
                        ),
                        arguments: Vec::new(),
                        configured: false,
                    },
                    operation_location,
                );
                let value = self.apply_decorators(
                    std::slice::from_ref(&model),
                    "Type",
                    &name,
                    located(ExprKind::Dict(fields), self.location(node)),
                    self.location(node),
                );
                Ok(located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::Trait,
                        declared_initializer: Some(DeclaredInitializerKind::Struct),
                        imported_name: None,
                        name,
                        type_parameters: vec![located("Self".to_owned(), self.location(node))],
                        type_parameter_bounds: vec![Vec::new()],
                        annotation: None,
                        value,
                    },
                    self.location(node),
                ))
            }
            Rule::ImplBinding => {
                let (type_parameters, type_parameter_bounds) = self
                    .rule_children(node)
                    .find(|child| self.rule(*child) == Some(Rule::TypeParameters))
                    .map(|parameters| self.type_parameters(parameters))
                    .transpose()?
                    .unwrap_or_default();
                let contracts = self
                    .rule_children(node)
                    .filter(|child| {
                        matches!(
                            self.rule(*child),
                            Some(Rule::Contract | Rule::ContractExpr | Rule::FunctionContract)
                        )
                    })
                    .collect::<Vec<_>>();
                let [trait_contract, target_contract] = contracts.as_slice() else {
                    return Err(self.error(node, "impl requires a trait and target type"));
                };
                let trait_value = self.contract_expression(*trait_contract)?;
                let target = self.contract_expression(*target_contract)?;
                let annotation = located(
                    ExprKind::Call {
                        callee: Box::new(trait_value),
                        arguments: vec![target],
                    },
                    self.location(node),
                );
                let mut fields = Vec::new();
                let mut names = std::collections::HashSet::new();
                for member in self
                    .rule_children(node)
                    .filter(|child| self.rule(*child) == Some(Rule::ImplMember))
                {
                    let name_node = self.first_token(member, Token::Identifier)?;
                    let member_name = self.identifier(name_node);
                    if !names.insert(member_name.value.clone()) {
                        return Err(self.error(
                            name_node,
                            format!("duplicate impl member {:?}", member_name.value),
                        ));
                    }
                    let colon = self.first_token(member, Token::Colon)?;
                    let value = self
                        .children(member)
                        .find(|child| {
                            self.is_expression(*child)
                                && self.cst.span(*child).start > self.cst.span(colon).start
                        })
                        .ok_or_else(|| self.error(member, "impl member has no value"))?;
                    fields.push(located(
                        DictFieldKind {
                            decorators: Vec::new(),
                            name: Some(member_name),
                            value: self.expression(value)?,
                        },
                        self.location(member),
                    ));
                }
                Ok(located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::Impl,
                        declared_initializer: None,
                        imported_name: None,
                        name,
                        type_parameters,
                        type_parameter_bounds,
                        annotation: Some(annotation),
                        value: located(ExprKind::Dict(fields), self.location(node)),
                    },
                    self.location(node),
                ))
            }
            Rule::ImportBinding => {
                let path = self
                    .rule_children(node)
                    .find(|child| self.rule(*child) == Some(Rule::StringLiteral))
                    .ok_or_else(|| self.error(node, "import has no path"))?;
                Ok(located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::Import,
                        declared_initializer: None,
                        imported_name: None,
                        name,
                        type_parameters: Vec::new(),
                        type_parameter_bounds: Vec::new(),
                        annotation: None,
                        value: located(
                            ExprKind::String(self.plain_string(path, "import path")?),
                            self.location(path),
                        ),
                    },
                    self.location(node),
                ))
            }
            _ => Err(self.error(node, "unexpected binding rule")),
        }
    }

    fn import_bindings(&self, node: NodeRef) -> Result<Vec<Binding>, Diagnostic> {
        let path = self
            .rule_children(node)
            .find(|child| self.rule(*child) == Some(Rule::StringLiteral))
            .ok_or_else(|| self.error(node, "import has no path"))?;
        let value = located(
            ExprKind::String(self.plain_string(path, "import path")?),
            self.location(path),
        );
        let selector = self
            .rule_children(node)
            .find(|child| self.rule(*child) == Some(Rule::ImportSelector))
            .ok_or_else(|| self.error(node, "import has no selector"))?;
        let mut bindings = Vec::new();
        if self.token_children(selector, Token::As).next().is_some() {
            let name_node = self
                .token_children(selector, Token::Identifier)
                .next()
                .ok_or_else(|| self.error(selector, "module import has no alias"))?;
            bindings.push(located(
                BindingData {
                    decorators: Vec::new(),
                    kind: BindingKind::Import,
                    declared_initializer: None,
                    imported_name: None,
                    name: self.identifier(name_node),
                    type_parameters: Vec::new(),
                    type_parameter_bounds: Vec::new(),
                    annotation: None,
                    value: value.clone(),
                },
                self.location(node),
            ));
        }
        if self.token_children(selector, Token::Star).next().is_some() {
            bindings.push(located(
                BindingData {
                    decorators: Vec::new(),
                    kind: BindingKind::OpenImport,
                    declared_initializer: None,
                    imported_name: None,
                    name: located(
                        format!("\0open:{}", self.cst.span(node).start),
                        self.location(node),
                    ),
                    type_parameters: Vec::new(),
                    type_parameter_bounds: Vec::new(),
                    annotation: None,
                    value: value.clone(),
                },
                self.location(node),
            ));
        }
        let Some(items) = self
            .rule_children(selector)
            .find(|child| self.rule(*child) == Some(Rule::ImportItems))
        else {
            return Ok(bindings);
        };
        bindings.extend(
            self.rule_children(items)
                .filter(|child| self.rule(*child) == Some(Rule::ImportItem))
                .map(|item| {
                    let names = self
                        .token_children(item, Token::Identifier)
                        .map(|name| self.identifier(name))
                        .collect::<Vec<_>>();
                    let imported_name = names
                        .first()
                        .cloned()
                        .ok_or_else(|| self.error(item, "import item has no exported name"))?;
                    let name = names
                        .get(1)
                        .cloned()
                        .unwrap_or_else(|| imported_name.clone());
                    Ok(located(
                        BindingData {
                            decorators: Vec::new(),
                            kind: BindingKind::Import,
                            declared_initializer: None,
                            imported_name: Some(Box::new(imported_name)),
                            name,
                            type_parameters: Vec::new(),
                            type_parameter_bounds: Vec::new(),
                            annotation: None,
                            value: value.clone(),
                        },
                        self.location(item),
                    ))
                })
                .collect::<Result<Vec<_>, _>>()?,
        );
        Ok(bindings)
    }

    fn export_bindings(&self, node: NodeRef) -> Result<Vec<Binding>, Diagnostic> {
        if let Some(binding_node) = self.rule_children(node).find(|child| {
            matches!(
                self.rule(*child),
                Some(
                    Rule::LetBinding
                        | Rule::DefBinding
                        | Rule::TypeBinding
                        | Rule::TraitBinding
                )
            )
        }) {
            if self.rule(binding_node) == Some(Rule::LetBinding) {
                return Err(self.error(binding_node, "export let is not supported; use export def"));
            }
            let binding = self.binding(binding_node)?;
            let local = binding.value.name.clone();
            let marker = located(
                BindingData {
                    decorators: Vec::new(),
                    kind: BindingKind::Export,
                    declared_initializer: None,
                    imported_name: Some(Box::new(local.clone())),
                    name: local.clone(),
                    type_parameters: Vec::new(),
                    type_parameter_bounds: Vec::new(),
                    annotation: None,
                    value: located(ExprKind::Variable(local), self.location(node)),
                },
                self.location(node),
            );
            return Ok(vec![binding, marker]);
        }
        let items = self
            .rule_children(node)
            .find(|child| self.rule(*child) == Some(Rule::ExportItems))
            .ok_or_else(|| self.error(node, "export has no binding or item list"))?;
        self.rule_children(items)
            .filter(|child| self.rule(*child) == Some(Rule::ExportItem))
            .map(|item| {
                let names = self
                    .token_children(item, Token::Identifier)
                    .map(|name| self.identifier(name))
                    .collect::<Vec<_>>();
                let local = names
                    .first()
                    .cloned()
                    .ok_or_else(|| self.error(item, "export item has no local name"))?;
                let public = names.get(1).cloned().unwrap_or_else(|| local.clone());
                Ok(located(
                    BindingData {
                        decorators: Vec::new(),
                        kind: BindingKind::Export,
                        declared_initializer: None,
                        imported_name: Some(Box::new(local.clone())),
                        name: public,
                        type_parameters: Vec::new(),
                        type_parameter_bounds: Vec::new(),
                        annotation: None,
                        value: located(ExprKind::Variable(local), self.location(item)),
                    },
                    self.location(item),
                ))
            })
            .collect()
    }

}
