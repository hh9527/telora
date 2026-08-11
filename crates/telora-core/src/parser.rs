use crate::ast::{
    BinaryOperator, Binding, BindingData, BindingKind, Block, BlockKind, ClosureParameter,
    Decorator, DecoratorKind, DictFieldKind, Expr, ExprKind, Identifier, MatchArm, MatchArmKind,
    OptionAction, Pattern, PatternKind, Program, ProgramKind, StringPartKind, StructPatternField,
    TypeArgument, TypeArgumentKind, UnaryOperator, located,
};
use crate::lexer::{FrontendError, SourceLocation};
use crate::source::{Diagnostic, Location, SourceDatabase, SourceId};
use crate::syntax::telora::lexer::Token;
use crate::syntax::telora::parser::{CstData, Node, NodeRef, Rule};
use std::collections::HashMap;

#[derive(Debug)]
pub struct FrontendParse {
    pub cst: CstData,
    pub options: Vec<OptionAction>,
    pub program: Option<Program>,
    pub recovered: RecoveredProgram,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct RecoveredProgram {
    pub location: Location,
    pub bindings: Vec<Binding>,
    pub result: Option<Expr>,
}

pub fn parse(source_name: &str, source: &str) -> Result<Program, FrontendError> {
    let mut sources = SourceDatabase::default();
    let source_id = sources.add(source_name, source);
    let parsed = parse_registered(&sources, source_id);
    if let Some(program) = parsed.program {
        return Ok(program);
    }
    Err(compatibility_error(
        &sources,
        source_id,
        &parsed.diagnostics,
    ))
}

pub fn parse_registered(sources: &SourceDatabase, source_id: SourceId) -> FrontendParse {
    let source = sources.get(source_id);
    let parsed = crate::syntax::telora::parse_document(source_id, source.text());
    let mut diagnostics = parsed.diagnostics;
    let lowerer = Lowerer::new(source_id, source.text(), &parsed.syntax);
    let options = match lowerer.option_actions() {
        Ok(options) => options,
        Err(diagnostic) => {
            push_unique_diagnostic(&mut diagnostics, diagnostic);
            Vec::new()
        }
    };
    let recovered = lowerer.recover_program(&mut diagnostics);
    let program = if diagnostics.is_empty() {
        match lowerer.program(options.clone()) {
            Ok(program) => Some(program),
            Err(diagnostic) => {
                diagnostics.push(diagnostic);
                None
            }
        }
    } else {
        None
    };
    FrontendParse {
        cst: parsed.syntax,
        options,
        program,
        recovered,
        diagnostics,
    }
}

fn compatibility_error(
    sources: &SourceDatabase,
    source_id: SourceId,
    diagnostics: &[Diagnostic],
) -> FrontendError {
    let diagnostic = diagnostics.first().expect("failed parse has a diagnostic");
    let offset = diagnostic
        .labels
        .first()
        .map_or(0, |label| label.location.start);
    let position = sources.get(source_id).position(offset);
    FrontendError::new(
        sources.get(source_id).name.as_ref(),
        SourceLocation {
            offset: offset as usize,
            line: position.line,
            column: position.column,
        },
        &diagnostic.message,
    )
}

struct Lowerer<'a> {
    source_id: SourceId,
    source: &'a crate::document::DocumentText,
    cst: &'a CstData,
}

enum BlockEntry {
    Binding(Binding),
    Destructure {
        pattern: Pattern,
        value: Expr,
        location: Location,
    },
    LetElse {
        pattern: Pattern,
        value: Expr,
        else_branch: Block,
        location: Location,
    },
}

enum CallArgument {
    Expression(Expr),
    Bare {
        node: NodeRef,
        location: Location,
    },
    Indexed {
        node: NodeRef,
        index: usize,
        location: Location,
    },
}

fn validate_option_literal(expression: &Expr) -> Result<(), Diagnostic> {
    let valid = match &expression.value {
        ExprKind::Int(_) | ExprKind::String(_) => true,
        ExprKind::Float(value) => value.is_finite(),
        ExprKind::Atom(_) => true,
        ExprKind::Array(values) => {
            for value in values {
                validate_option_literal(value)?;
            }
            true
        }
        ExprKind::Dict(fields) => {
            for field in fields {
                if field.value.name.is_none() {
                    return Err(Diagnostic::error(
                        "option Dicts cannot contain spread fields",
                        field.location,
                    ));
                }
                if !field.value.decorators.is_empty() {
                    return Err(Diagnostic::error(
                        "option fields cannot have decorators",
                        field.location,
                    ));
                }
                validate_option_literal(&field.value.value)?;
            }
            true
        }
        ExprKind::Call { callee, arguments }
            if matches!(callee.value, ExprKind::Atom(_)) && arguments.len() == 1 =>
        {
            validate_option_literal(&arguments[0])?;
            true
        }
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(Diagnostic::error(
            "option accepts only immediate values",
            expression.location,
        ))
    }
}

fn valid_option_key(key: &str) -> bool {
    let mut segments = key.split('.');
    let Some(first) = segments.next() else {
        return false;
    };
    let rest = segments.collect::<Vec<_>>();
    !rest.is_empty()
        && std::iter::once(first).chain(rest).all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|character| character.is_ascii_lowercase())
                && characters.all(|character| {
                    character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
                })
        })
}

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

    fn program(&self, options: Vec<OptionAction>) -> Result<Program, Diagnostic> {
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
                "a module cannot mix explicit exports with a final expression",
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
                options,
                body,
                authored_result,
            },
            self.location(root),
        ))
    }

    fn option_actions(&self) -> Result<Vec<OptionAction>, Diagnostic> {
        let body = self
            .rule_children(NodeRef::ROOT)
            .find(|node| self.rule(*node) == Some(Rule::Body))
            .ok_or_else(|| self.error(NodeRef::ROOT, "program has no body"))?;
        let mut options = Vec::new();
        for child in self.children(body) {
            let node = if self.rule(child) == Some(Rule::Binding) {
                self.first_rule(child)
                    .ok_or_else(|| self.error(child, "empty binding"))?
            } else {
                child
            };
            if self.rule(node) != Some(Rule::OptionBinding) {
                continue;
            }
            let key_node = self
                .rule_children(node)
                .find(|child| self.rule(*child) == Some(Rule::StringLiteral))
                .ok_or_else(|| self.error(node, "option has no key"))?;
            let key = self.plain_string(key_node, "option key")?;
            if !valid_option_key(&key) {
                return Err(self.error(
                    key_node,
                    "option key must contain lower-case dotted segments",
                ));
            }
            let value_node = self
                .children(node)
                .find(|child| {
                    self.is_expression(*child)
                        && self.cst.span(*child).start >= self.cst.span(key_node).end
                })
                .ok_or_else(|| self.error(node, "option has no value"))?;
            let value = self.expression(value_node)?;
            validate_option_literal(&value)?;
            options.push(OptionAction {
                key: located(key, self.location(key_node)),
                value,
                location: self.location(node),
            });
        }
        Ok(options)
    }

    fn recover_program(&self, diagnostics: &mut Vec<Diagnostic>) -> RecoveredProgram {
        use crate::syntax::telora::ast::{AstNode, Program as SyntaxProgram};

        let root = SyntaxProgram::root(self.cst);
        let mut bindings = Vec::new();
        let mut result = None;
        if let Some(body) = root.body() {
            for binding in body.bindings() {
                let node = binding.syntax().node_ref();
                if matches!(binding, crate::syntax::telora::ast::Binding::Option(_)) {
                    continue;
                }
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
                Some(Rule::OptionBinding) => {
                    if allow_destructuring {
                        return Err(self.error(
                            child,
                            "option declarations are allowed only at module top level",
                        ));
                    }
                }
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
                    } else if self.rule(inner) == Some(Rule::OptionBinding) {
                        if allow_destructuring {
                            return Err(self.error(
                                inner,
                                "option declarations are allowed only at module top level",
                            ));
                        }
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

    fn binding(&self, node: NodeRef) -> Result<Binding, Diagnostic> {
        let identifiers = self
            .token_children(node, Token::Identifier)
            .collect::<Vec<_>>();
        let name_node = identifiers
            .first()
            .copied()
            .ok_or_else(|| self.error(node, "binding has no name"))?;
        let name = self.identifier(name_node);
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
                        imported_name: None,
                        name,
                        type_parameters: Vec::new(),
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
                let type_parameters = self
                    .rule_children(scheme)
                    .find(|child| self.rule(*child) == Some(Rule::TypeParameters))
                    .map(|parameters| {
                        self.token_children(parameters, Token::Identifier)
                            .map(|parameter| {
                                located(self.text(parameter).into_owned(), self.location(parameter))
                            })
                            .collect()
                    })
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
                        imported_name: None,
                        name,
                        type_parameters,
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
                let type_parameters = self
                    .rule_children(scheme)
                    .find(|child| self.rule(*child) == Some(Rule::TypeParameters))
                    .map(|parameters| {
                        self.token_children(parameters, Token::Identifier)
                            .map(|parameter| {
                                located(self.text(parameter).into_owned(), self.location(parameter))
                            })
                            .collect()
                    })
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
                        imported_name: None,
                        name,
                        type_parameters,
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
                        imported_name: None,
                        name,
                        type_parameters: Vec::new(),
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
                let type_parameters: Vec<Identifier> = scheme
                    .and_then(|scheme| {
                        self.rule_children(scheme)
                            .find(|child| self.rule(*child) == Some(Rule::TypeParameters))
                    })
                    .map(|parameters| {
                        self.token_children(parameters, Token::Identifier)
                            .map(|parameter| {
                                located(self.text(parameter).into_owned(), self.location(parameter))
                            })
                            .collect()
                    })
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
                        imported_name: None,
                        name,
                        type_parameters,
                        annotation,
                        value,
                    },
                    self.location(node),
                ))
            }
            Rule::TypeBinding => {
                let decorators = self.decorators(node)?;
                let type_parameters = self
                    .rule_children(node)
                    .find(|child| self.rule(*child) == Some(Rule::TypeParameters))
                    .map(|parameters| {
                        self.token_children(parameters, Token::Identifier)
                            .map(|parameter| {
                                located(self.text(parameter).into_owned(), self.location(parameter))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let equal = self.first_token(node, Token::Equal)?;
                let start = self.cst.span(equal).start;
                let value = self.expression(
                    self.children(node)
                        .find(|child| {
                            self.is_expression(*child) && self.cst.span(*child).start > start
                        })
                        .ok_or_else(|| self.error(node, "type has no value"))?,
                )?;
                let value =
                    self.apply_decorators(&decorators, "Type", &name, value, self.location(node));
                Ok(located(
                    BindingData {
                        decorators,
                        kind: BindingKind::Type,
                        imported_name: None,
                        name,
                        type_parameters,
                        annotation: None,
                        value,
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
                        imported_name: None,
                        name,
                        type_parameters: Vec::new(),
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
                    imported_name: None,
                    name: self.identifier(name_node),
                    type_parameters: Vec::new(),
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
                    imported_name: None,
                    name: located(
                        format!("\0open:{}", self.cst.span(node).start),
                        self.location(node),
                    ),
                    type_parameters: Vec::new(),
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
                            imported_name: Some(Box::new(imported_name)),
                            name,
                            type_parameters: Vec::new(),
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
                Some(Rule::LetBinding | Rule::DefBinding | Rule::TypeBinding)
            )
        }) {
            let binding = self.binding(binding_node)?;
            let local = binding.value.name.clone();
            let marker = located(
                BindingData {
                    decorators: Vec::new(),
                    kind: BindingKind::Export,
                    imported_name: Some(Box::new(local.clone())),
                    name: local.clone(),
                    type_parameters: Vec::new(),
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
                        imported_name: Some(Box::new(local.clone())),
                        name: public,
                        type_parameters: Vec::new(),
                        annotation: None,
                        value: located(ExprKind::Variable(local), self.location(item)),
                    },
                    self.location(item),
                ))
            })
            .collect()
    }

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
                    self.text(node)
                        .parse()
                        .map_err(|_| self.error(node, "invalid Float literal"))?,
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
            Rule::FloatExpr => ExprKind::Float(
                self.text(self.first_token(node, Token::Float)?)
                    .parse()
                    .map_err(|_| self.error(node, "invalid Float literal"))?,
            ),
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
                    let value = self.apply_decorators(
                        &decorators,
                        "Field",
                        &name,
                        value,
                        self.location(field),
                    );
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
                operator: located(
                    UnaryOperator::Negate,
                    self.location(self.first_token(node, Token::Minus)?),
                ),
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
                let comparison = self.token_children(node, Token::Less).next().is_some()
                    || self
                        .token_children(node, Token::EqualEqual)
                        .next()
                        .is_some();
                if comparison
                    && self.children(node).any(|child| {
                        self.rule(child) == Some(Rule::BinaryExpr)
                            && (self.token_children(child, Token::Less).next().is_some()
                                || self
                                    .token_children(child, Token::EqualEqual)
                                    .next()
                                    .is_some())
                    })
                {
                    return Err(self.error(
                        node,
                        "comparison operators do not associate; add parentheses",
                    ));
                }
                let values = self.expression_children(node)?;
                let (operator, operator_node) =
                    if let Some(operator) = self.token_children(node, Token::Plus).next() {
                        (BinaryOperator::Add, operator)
                    } else if let Some(operator) = self.token_children(node, Token::Minus).next() {
                        (BinaryOperator::Subtract, operator)
                    } else if let Some(operator) = self.token_children(node, Token::Star).next() {
                        (BinaryOperator::Multiply, operator)
                    } else if let Some(operator) = self.token_children(node, Token::Slash).next() {
                        (BinaryOperator::Divide, operator)
                    } else if let Some(operator) = self.token_children(node, Token::Less).next() {
                        (BinaryOperator::LessThan, operator)
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
                    left: Box::new(values[0].clone()),
                    right: Box::new(values[1].clone()),
                }
            }
            Rule::FieldExpr => {
                let receiver = self
                    .children(node)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(node, "field access has no receiver"))?;
                let field = self
                    .token_children(node, Token::Identifier)
                    .last()
                    .ok_or_else(|| self.error(node, "field access has no field"))?;
                ExprKind::Field {
                    receiver: Box::new(self.expression(receiver)?),
                    field: self.identifier(field),
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
                ExprKind::Call {
                    callee: Box::new(callee),
                    arguments,
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
                ExprKind::If {
                    condition: Box::new(condition),
                    then_branch: self.block_body(blocks[0])?,
                    else_branch: self.block_body(blocks[1])?,
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
                    else_branch: self.block_body(blocks[1])?,
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
                if matches!(
                    name_text.as_ref(),
                    "blame" | "panic" | "raise" | "emit_info" | "emit_warn" | "emit_error" | "fail"
                ) {
                    let bang = self.first_token(node, Token::Bang)?;
                    let arguments = self
                        .children(node)
                        .filter(|child| {
                            self.is_expression(*child)
                                && self.cst.span(*child).start > self.cst.span(bang).end
                        })
                        .map(|argument| self.expression(argument))
                        .collect::<Result<Vec<_>, _>>()?;
                    if name_text == "blame" {
                        self.lower_blame(arguments, node)
                    } else if matches!(
                        name_text.as_ref(),
                        "emit_info" | "emit_warn" | "emit_error" | "fail"
                    ) {
                        self.lower_diagnostic_convenience(&name_text, arguments, node)
                    } else {
                        if arguments.len() != 1 {
                            return Err(self.error(
                                node,
                                format!(
                                    "{name_text}! expects exactly one argument, found {}",
                                    arguments.len()
                                ),
                            ));
                        }
                        let argument = Box::new(arguments.into_iter().next().unwrap());
                        let kind = if name_text == "panic" {
                            ExprKind::Panic { message: argument }
                        } else {
                            ExprKind::Raise { error: argument }
                        };
                        Ok(located(kind, self.location(node)))
                    }
                } else if matches!(name_text.as_ref(), "file" | "line") {
                    Err(self.error(
                        name,
                        format!("{name_text}! is reserved but not implemented"),
                    ))
                } else {
                    Err(self.error(name, format!("unknown contextual intrinsic {name_text}!")))
                }
            }
            _ => Err(self.error(node, "contextual intrinsic has no supported name")),
        }
    }

    fn lower_blame(&self, arguments: Vec<Expr>, node: NodeRef) -> Result<Expr, Diagnostic> {
        if arguments.is_empty() {
            return Err(self.error(
                node,
                "blame! expects a message followed by zero or more subjects",
            ));
        }
        let location = self.location(node);
        let mut arguments = arguments.into_iter();
        let message = arguments.next().expect("blame message was checked");
        let subjects = arguments.collect::<Vec<_>>();
        let data = match subjects.len() {
            1 => subjects.into_iter().next().expect("one blame subject"),
            _ => located(ExprKind::Tuple(subjects), location),
        };
        let rule = located(ExprKind::String("blame!".into()), location);
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

    fn lower_diagnostic_convenience(
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
        let blame = self.lower_blame(arguments, node)?;
        if name == "fail" {
            return Ok(located(
                ExprKind::Raise {
                    error: Box::new(blame),
                },
                location,
            ));
        }
        let severity = match name {
            "emit_info" => "Info",
            "emit_warn" => "Warn",
            "emit_error" => "Error",
            _ => unreachable!("diagnostic convenience name was checked"),
        };
        Ok(located(
            ExprKind::Call {
                callee: Box::new(located(
                    ExprKind::Variable(located("report".into(), location)),
                    location,
                )),
                arguments: vec![located(ExprKind::Atom(severity.into()), location), blame],
            },
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
                    self.text(node)
                        .parse()
                        .map_err(|_| self.error(node, "invalid Float pattern"))?,
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
            Rule::FloatPattern => PatternKind::Float(
                self.text(self.first_token(node, Token::Float)?)
                    .parse()
                    .map_err(|_| self.error(node, "invalid Float pattern"))?,
            ),
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
                let name = self.identifier(self.first_token(node, Token::Identifier)?);
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
                    Ok(located(ExprKind::Variable(name), location))
                } else {
                    Ok(located(
                        ExprKind::Call {
                            callee: Box::new(located(ExprKind::Variable(name), location)),
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

    fn apply_decorators(
        &self,
        decorators: &[Decorator],
        kind: &str,
        name: &Identifier,
        mut value: Expr,
        target_location: Location,
    ) -> Expr {
        let context = located(
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
        );
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

    fn expression_children(&self, node: NodeRef) -> Result<Vec<Expr>, Diagnostic> {
        self.children(node)
            .filter(|child| self.is_expression(*child))
            .map(|child| self.expression(child))
            .collect()
    }

    fn type_arguments(&self, node: NodeRef) -> Result<Vec<TypeArgument>, Diagnostic> {
        self.rule_children(node)
            .filter(|child| self.rule(*child) == Some(Rule::TypeArgument))
            .map(|argument| {
                if let Some(placeholder) = self.token_children(argument, Token::Placeholder).next()
                {
                    return Ok(located(TypeArgumentKind::Infer, self.location(placeholder)));
                }
                let expression = self
                    .children(argument)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(argument, "type argument has no expression"))?;
                let expression = self.expression(expression)?;
                let location = expression.location;
                Ok(located(TypeArgumentKind::Explicit(expression), location))
            })
            .collect()
    }

    fn section_expression(
        &self,
        callee: Expr,
        arguments_node: Option<NodeRef>,
        section_node: NodeRef,
        location: Location,
    ) -> Result<Expr, Diagnostic> {
        let Some(arguments_node) = arguments_node else {
            return Err(self.error(section_node, "call section has no arguments"));
        };
        let arguments = self
            .rule_children(arguments_node)
            .filter(|child| self.rule(*child) == Some(Rule::Argument))
            .map(|argument| self.call_argument(argument))
            .collect::<Result<Vec<_>, _>>()?;
        elaborate_call_section(callee, arguments, section_node, location)
            .map_err(|(node, message)| self.error(node, message))
    }

    fn call_argument(&self, node: NodeRef) -> Result<CallArgument, Diagnostic> {
        if let Some(placeholder) = self.token_children(node, Token::Placeholder).next() {
            return Ok(CallArgument::Bare {
                node: placeholder,
                location: self.location(placeholder),
            });
        }
        if let Some(placeholder) = self.token_children(node, Token::IndexedPlaceholder).next() {
            let text = self.text(placeholder);
            let index = text[1..].parse::<usize>().map_err(|_| {
                self.error(placeholder, "placeholder index exceeds the supported range")
            })?;
            return Ok(CallArgument::Indexed {
                node: placeholder,
                index,
                location: self.location(placeholder),
            });
        }
        let expression = self
            .children(node)
            .find(|child| self.is_expression(*child))
            .ok_or_else(|| self.error(node, "call argument has no expression"))?;
        Ok(CallArgument::Expression(self.expression(expression)?))
    }
    fn is_expression(&self, node: NodeRef) -> bool {
        matches!(
            self.cst.get(node),
            Node::Token(
                Token::Int | Token::Float | Token::Bytes | Token::Atom | Token::Identifier,
                _
            )
        ) || matches!(
            self.rule(node),
            Some(
                Rule::Expression
                    | Rule::Primary
                    | Rule::Braced
                    | Rule::ArrayExpr
                    | Rule::AtomExpr
                    | Rule::BinaryExpr
                    | Rule::Block
                    | Rule::BytesExpr
                    | Rule::CallExpr
                    | Rule::Closure
                    | Rule::DictExpr
                    | Rule::FieldExpr
                    | Rule::FloatExpr
                    | Rule::FunctionContract
                    | Rule::IfExpr
                    | Rule::IfLetExpr
                    | Rule::InterpreterIntrinsic
                    | Rule::NamedIntrinsic
                    | Rule::LegacyInterpreterExpr
                    | Rule::IntExpr
                    | Rule::MatchExpr
                    | Rule::ParenExpr
                    | Rule::PipelineExpr
                    | Rule::PropagateExpr
                    | Rule::ReturnExpr
                    | Rule::SectionExpr
                    | Rule::SpreadExpr
                    | Rule::StringExpr
                    | Rule::TypeApplyExpr
                    | Rule::UnaryExpr
                    | Rule::VariableExpr
            )
        )
    }
    fn is_pattern(&self, node: NodeRef) -> bool {
        matches!(
            self.cst.get(node),
            Node::Token(
                Token::Identifier | Token::Placeholder | Token::Int | Token::Float | Token::Atom,
                _
            )
        ) || matches!(
            self.rule(node),
            Some(
                Rule::Pattern
                    | Rule::AtomPattern
                    | Rule::FloatPattern
                    | Rule::IdentifierPattern
                    | Rule::IntPattern
                    | Rule::StringPattern
                    | Rule::TaggedPattern
                    | Rule::TuplePattern
                    | Rule::StructPattern
            )
        )
    }
    fn children(&self, node: NodeRef) -> impl Iterator<Item = NodeRef> + '_ {
        self.cst.children(node)
    }
    fn rule_children(&self, node: NodeRef) -> impl Iterator<Item = NodeRef> + '_ {
        self.children(node)
            .filter(|child| matches!(self.cst.get(*child), Node::Rule(..)))
    }
    fn token_children(&self, node: NodeRef, token: Token) -> impl Iterator<Item = NodeRef> + '_ {
        self.children(node).filter(
            move |child| matches!(self.cst.get(*child), Node::Token(found, _) if found == token),
        )
    }
    fn first_rule(&self, node: NodeRef) -> Option<NodeRef> {
        self.rule_children(node).next()
    }
    fn expression_head(&self, mut node: NodeRef) -> NodeRef {
        while matches!(
            self.rule(node),
            Some(Rule::Expression | Rule::Primary | Rule::Braced)
        ) {
            let Some(child) = self.first_rule(node) else {
                break;
            };
            node = child;
        }
        node
    }
    fn first_token(&self, node: NodeRef, token: Token) -> Result<NodeRef, Diagnostic> {
        self.token_children(node, token)
            .next()
            .ok_or_else(|| self.error(node, format!("missing {token:?}")))
    }
    fn rule(&self, node: NodeRef) -> Option<Rule> {
        match self.cst.get(node) {
            Node::Rule(rule, _) => Some(rule),
            Node::Token(..) => None,
        }
    }
    fn location(&self, node: NodeRef) -> Location {
        Location::from_usize(self.source_id, self.cst.span(node))
            .expect("CST span fits registered source")
    }
    fn identifier(&self, node: NodeRef) -> Identifier {
        located(self.text(node).into_owned(), self.location(node))
    }
    fn text(&self, node: NodeRef) -> std::borrow::Cow<'_, str> {
        self.source
            .slice(
                crate::source::TextRange::from_usize(self.cst.span(node))
                    .expect("CST span fits registered source"),
            )
            .expect("CST span is a valid source slice")
    }
    fn error(&self, node: NodeRef, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.location(node))
    }

    fn string_expression(&self, node: NodeRef) -> Result<Expr, Diagnostic> {
        let text_node = self
            .rule_children(node)
            .find(|child| {
                matches!(
                    self.rule(*child),
                    Some(Rule::StringLiteral | Rule::ConcatExpression)
                )
            })
            .ok_or_else(|| self.error(node, "string expression has no text"))?;
        let mut components = Vec::new();
        self.collect_string_components(text_node, &mut components);
        if self.rule(text_node) == Some(Rule::StringLiteral) {
            return Ok(located(
                ExprKind::String(self.decode_string_components(&components)?),
                self.location(node),
            ));
        }

        let mut parts = Vec::new();
        for component in components {
            if self.rule(component) == Some(Rule::Interpolation) {
                let expression_node = self
                    .children(component)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(component, "interpolation has no expression"))?;
                let expression = self.expression(expression_node)?;
                let location = expression.location;
                parts.push(located(StringPartKind::Expression(expression), location));
            } else {
                parts.push(located(
                    StringPartKind::Text(self.decode_string_component(component)?),
                    self.location(component),
                ));
            }
        }
        Ok(located(
            ExprKind::InterpolatedString(parts),
            self.location(node),
        ))
    }

    fn plain_string(&self, node: NodeRef, context: &str) -> Result<String, Diagnostic> {
        let mut components = Vec::new();
        self.collect_string_components(node, &mut components);
        let _ = context;
        self.decode_string_components(&components)
    }

    fn collect_string_components(&self, node: NodeRef, output: &mut Vec<NodeRef>) {
        match self.cst.get(node) {
            Node::Token(Token::StringText | Token::EscapeSequence | Token::RawString, _) => {
                output.push(node)
            }
            Node::Rule(Rule::Interpolation, _) => output.push(node),
            Node::Token(..) => {}
            Node::Rule(..) => {
                for child in self.children(node) {
                    self.collect_string_components(child, output);
                }
            }
        }
    }

    fn decode_string_components(&self, components: &[NodeRef]) -> Result<String, Diagnostic> {
        let mut output = String::new();
        for component in components {
            output.push_str(&self.decode_string_component(*component)?);
        }
        Ok(output)
    }

    fn decode_string_component(&self, node: NodeRef) -> Result<String, Diagnostic> {
        match self.cst.get(node) {
            Node::Token(Token::StringText, _) => Ok(self.text(node).into_owned()),
            Node::Token(Token::EscapeSequence, _) => self.decode_escape(node),
            Node::Token(Token::RawString, _) => self.decode_raw_string(node),
            _ => Err(self.error(node, "expected string text or escape")),
        }
    }

    fn decode_escape(&self, node: NodeRef) -> Result<String, Diagnostic> {
        let text = self.text(node);
        let escaped = &text[1..];
        if escaped.starts_with(['\n', '\r']) {
            return Ok(String::new());
        }
        let decoded = match escaped {
            "0" => "\0".to_owned(),
            "n" => "\n".to_owned(),
            "r" => "\r".to_owned(),
            "t" => "\t".to_owned(),
            "\"" => "\"".to_owned(),
            "`" => "`".to_owned(),
            "\\" => "\\".to_owned(),
            value if value.starts_with('x') => {
                let byte = u8::from_str_radix(&value[1..], 16)
                    .map_err(|_| self.error(node, "invalid ASCII string escape"))?;
                if !byte.is_ascii() {
                    return Err(self.error(node, "\\x string escape must be ASCII"));
                }
                char::from(byte).to_string()
            }
            value if value.starts_with("u{") => {
                let digits = &value[2..value.len() - 1];
                let scalar = u32::from_str_radix(digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| self.error(node, "invalid Unicode scalar escape"))?;
                scalar.to_string()
            }
            _ => return Err(self.error(node, format!("unsupported escape \\{escaped}"))),
        };
        Ok(decoded)
    }

    fn decode_raw_string(&self, node: NodeRef) -> Result<String, Diagnostic> {
        let text = self.text(node);
        let hashes = text[1..].bytes().take_while(|byte| *byte == b'#').count();
        if hashes > 255 {
            return Err(self.error(node, "raw String delimiter exceeds 255 # characters"));
        }
        let opener = hashes + 2;
        let terminator = format!("\"{}", "#".repeat(hashes));
        if text.len() < opener + terminator.len() || !text.ends_with(&terminator) {
            return Err(self.error(node, "unterminated raw String"));
        }
        Ok(text[opener..text.len() - terminator.len()].to_owned())
    }

    fn decode_telora_string(&self, node: NodeRef) -> Result<String, Diagnostic> {
        let text = self.text(node);
        let quoted = text.strip_prefix('b').unwrap_or(&text);
        let mut chars = quoted[1..quoted.len() - 1].chars();
        let mut output = String::new();
        while let Some(character) = chars.next() {
            if character != '\\' {
                output.push(character);
                continue;
            }
            output.push(match chars.next() {
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                Some('"') => '"',
                Some('\\') => '\\',
                Some(other) => {
                    return Err(self.error(node, format!("unsupported escape \\{other}")));
                }
                None => return Err(self.error(node, "unterminated string escape")),
            });
        }
        Ok(output)
    }
}

fn synthesize_export_record(bindings: &[Binding], location: Location) -> Expr {
    let fields = bindings
        .iter()
        .filter(|binding| binding.value.kind == BindingKind::Export)
        .map(|binding| {
            let local = binding
                .value
                .imported_name
                .as_deref()
                .expect("export markers retain their local name")
                .clone();
            located(
                DictFieldKind {
                    decorators: Vec::new(),
                    name: Some(binding.value.name.clone()),
                    value: located(ExprKind::Variable(local), binding.location),
                },
                binding.location,
            )
        })
        .collect();
    located(ExprKind::Dict(fields), location)
}

fn push_unique_diagnostic(diagnostics: &mut Vec<Diagnostic>, diagnostic: Diagnostic) {
    let location = diagnostic.labels.first().map(|label| label.location);
    if diagnostics.iter().any(|existing| {
        existing.message == diagnostic.message
            && existing.labels.first().map(|label| label.location) == location
    }) {
        return;
    }
    diagnostics.push(diagnostic);
}

fn elaborate_pipeline(location: Location, left: Expr, right: Expr) -> Expr {
    located(
        ExprKind::Call {
            callee: Box::new(right),
            arguments: vec![left],
        },
        location,
    )
}

const MAX_PLACEHOLDER_PARAMETERS: usize = u16::MAX as usize;

fn elaborate_call_section(
    callee: Expr,
    arguments: Vec<CallArgument>,
    section_node: NodeRef,
    location: Location,
) -> Result<Expr, (NodeRef, String)> {
    let first_bare = arguments.iter().find_map(|argument| match argument {
        CallArgument::Bare { node, .. } => Some(*node),
        _ => None,
    });
    let first_indexed = arguments.iter().find_map(|argument| match argument {
        CallArgument::Indexed { node, .. } => Some(*node),
        _ => None,
    });
    if first_bare.is_some()
        && let Some(indexed) = first_indexed
    {
        return Err((
            indexed,
            "cannot mix '_' and indexed placeholders in one call".into(),
        ));
    }

    if first_bare.is_none() && first_indexed.is_none() {
        return Err((
            section_node,
            "call section requires at least one placeholder".into(),
        ));
    }

    let mut parameter_locations = Vec::new();
    if first_bare.is_some() {
        parameter_locations.extend(arguments.iter().filter_map(|argument| match argument {
            CallArgument::Bare { location, .. } => Some(Some(*location)),
            _ => None,
        }));
    } else {
        let max = arguments
            .iter()
            .filter_map(|argument| match argument {
                CallArgument::Indexed { index, .. } => Some(*index),
                _ => None,
            })
            .max()
            .expect("indexed placeholder exists");
        if max >= MAX_PLACEHOLDER_PARAMETERS {
            let node = arguments
                .iter()
                .find_map(|argument| match argument {
                    CallArgument::Indexed { node, index, .. } if *index == max => Some(*node),
                    _ => None,
                })
                .expect("maximum placeholder has a node");
            return Err((
                node,
                format!(
                    "placeholder index exceeds the limit of {} parameters",
                    MAX_PLACEHOLDER_PARAMETERS
                ),
            ));
        }
        parameter_locations.resize(max + 1, None);
        for argument in &arguments {
            if let CallArgument::Indexed {
                index, location, ..
            } = argument
            {
                parameter_locations[*index].get_or_insert(*location);
            }
        }
        if let Some(missing) = parameter_locations.iter().position(Option::is_none) {
            return Err((
                first_indexed.expect("indexed placeholder exists"),
                format!("indexed placeholders are missing _{missing}"),
            ));
        }
    }

    let parameter_locations = parameter_locations
        .into_iter()
        .map(|location| location.expect("placeholder location was assigned"))
        .collect::<Vec<_>>();
    let parameters = parameter_locations
        .iter()
        .enumerate()
        .map(|(index, location)| ClosureParameter {
            name: located(placeholder_parameter(index), *location),
            annotation: None,
        })
        .collect::<Vec<_>>();
    let mut next_bare = 0usize;
    let arguments = arguments
        .into_iter()
        .map(|argument| match argument {
            CallArgument::Expression(expression) => expression,
            CallArgument::Bare { location, .. } => {
                let index = next_bare;
                next_bare += 1;
                placeholder_variable(index, location)
            }
            CallArgument::Indexed {
                index, location, ..
            } => placeholder_variable(index, location),
        })
        .collect();
    let call = located(
        ExprKind::Call {
            callee: Box::new(callee),
            arguments,
        },
        location,
    );
    Ok(located(
        ExprKind::Closure {
            parameters,
            result_annotation: None,
            body: located(
                BlockKind {
                    bindings: Vec::new(),
                    result: Box::new(call),
                },
                location,
            ),
        },
        location,
    ))
}

fn placeholder_parameter(index: usize) -> String {
    format!("\0telora_placeholder_{index}")
}

struct InterpreterSyntaxPlan {
    witness_count: usize,
    parameters: Vec<Option<usize>>,
}

fn interpreter_syntax_plan(
    type_parameters: &[Identifier],
    contract: &Expr,
) -> Option<InterpreterSyntaxPlan> {
    let (outer_parameters, outer_result) = function_contract_parts(contract)?;
    let mut witnesses = HashMap::new();
    for (index, witness) in outer_parameters.iter().enumerate() {
        let ExprKind::Call { callee, arguments } = &witness.value else {
            return None;
        };
        if !is_variable(callee, "TypeOf") {
            return None;
        }
        let [argument] = arguments.as_slice() else {
            return None;
        };
        let ExprKind::Variable(parameter) = &argument.value else {
            return None;
        };
        if !type_parameters
            .iter()
            .any(|candidate| candidate.value == parameter.value)
            || witnesses.insert(parameter.value.clone(), index).is_some()
        {
            return None;
        }
    }
    if witnesses.len() != type_parameters.len() {
        return None;
    }
    let (inner_parameters, _) = function_contract_parts(outer_result)?;
    let parameters = inner_parameters
        .iter()
        .map(|parameter| match &parameter.value {
            ExprKind::Variable(name) => witnesses.get(&name.value).copied(),
            _ => None,
        })
        .collect();
    Some(InterpreterSyntaxPlan {
        witness_count: outer_parameters.len(),
        parameters,
    })
}

fn function_contract_parts(contract: &Expr) -> Option<(&[Expr], &Expr)> {
    let ExprKind::Call { callee, arguments } = &contract.value else {
        return None;
    };
    if !is_variable(callee, "Func") {
        return None;
    }
    let [parameters, result] = arguments.as_slice() else {
        return None;
    };
    let ExprKind::Array(parameters) = &parameters.value else {
        return None;
    };
    Some((parameters, result))
}

fn is_variable(expression: &Expr, expected: &str) -> bool {
    matches!(&expression.value, ExprKind::Variable(name) if name.value == expected)
}

fn interpreter_expansion(operand: Expr, location: Location, plan: &InterpreterSyntaxPlan) -> Expr {
    let variable = |name: &str| {
        located(
            ExprKind::Variable(located(name.to_owned(), location)),
            location,
        )
    };
    let pack = |witness_index: usize, value_name: &str| {
        located(
            ExprKind::Call {
                callee: Box::new(variable("\0telora_pack_dyn")),
                arguments: vec![
                    variable(&format!("\0telora_interpreter_type_{witness_index}")),
                    variable(value_name),
                ],
            },
            location,
        )
    };
    let value_names = (0..plan.parameters.len())
        .map(|index| format!("\0telora_interpreter_value_{index}"))
        .collect::<Vec<_>>();
    let call = located(
        ExprKind::Call {
            callee: Box::new(operand),
            arguments: plan
                .parameters
                .iter()
                .zip(&value_names)
                .map(|(witness, value)| {
                    witness.map_or_else(|| variable(value), |index| pack(index, value))
                })
                .collect(),
        },
        location,
    );
    let parameter = |name: &str| ClosureParameter {
        name: located(name.to_owned(), location),
        annotation: None,
    };
    let inner = located(
        ExprKind::Closure {
            parameters: value_names.iter().map(|name| parameter(name)).collect(),
            result_annotation: None,
            body: located(
                BlockKind {
                    bindings: Vec::new(),
                    result: Box::new(call),
                },
                location,
            ),
        },
        location,
    );
    located(
        ExprKind::Closure {
            parameters: (0..plan.witness_count)
                .map(|index| parameter(&format!("\0telora_interpreter_type_{index}")))
                .collect(),
            result_annotation: None,
            body: located(
                BlockKind {
                    bindings: Vec::new(),
                    result: Box::new(inner),
                },
                location,
            ),
        },
        location,
    )
}

fn placeholder_variable(index: usize, location: Location) -> Expr {
    located(
        ExprKind::Variable(located(placeholder_parameter(index), location)),
        location,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Located;

    #[test]
    fn accepts_hash_comments_and_shebangs() {
        let program = parse(
            "script.telora",
            "#!/usr/bin/env -S telora run\nlet value = 42; # answer\nvalue",
        )
        .unwrap();
        assert_eq!(program.value.body.value.bindings.len(), 1);
    }

    #[test]
    fn lowers_directly_from_cst_with_spans_and_precedence() {
        let mut sources = SourceDatabase::default();
        let id = sources.add("test.telora", "let x = 1; x == 2");
        let parsed = parse_registered(&sources, id);
        assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
        let program = parsed.program.unwrap();
        assert_eq!(program.location.range(), 0..17);
        assert_eq!(program.value.body.value.bindings[0].location.range(), 0..10);
        assert!(matches!(
            &program.value.body.value.result.value,
            ExprKind::Binary {
                operator: Located {
                    value: BinaryOperator::Equal,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn lowers_tagged_patterns() {
        let program = parse("test.telora", "match 'Some(1) { 'Some(value) => value }").unwrap();
        let ExprKind::Match { arms, .. } = &program.value.body.value.result.value else {
            panic!("expected match");
        };
        assert!(
            matches!(
                &arms[0].value.pattern.value,
                PatternKind::Tagged { payload, .. }
                    if matches!(payload.value, PatternKind::Binding(_))
            ),
            "{:?}",
            arms[0].value.pattern.value
        );
    }

    #[test]
    fn lowers_shorthand_and_nested_struct_patterns() {
        let program = parse(
            "test.telora",
            "match user { { name, address: { city }, } => (name, city) }",
        )
        .unwrap();
        let ExprKind::Match { arms, .. } = &program.value.body.value.result.value else {
            panic!("expected match");
        };
        let PatternKind::Struct(fields) = &arms[0].value.pattern.value else {
            panic!("expected Struct pattern");
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name.value, "name");
        assert!(matches!(fields[0].pattern.value, PatternKind::Binding(_)));
        assert!(matches!(fields[1].pattern.value, PatternKind::Struct(_)));
    }

    #[test]
    fn elaborates_local_destructuring_let_into_an_irrefutable_match() {
        let program = parse(
            "test.telora",
            "{ let (left, {name}) = (1, {name: \"Ada\"}); (left, name) }",
        )
        .unwrap();
        let ExprKind::Block(block) = &program.value.body.value.result.value else {
            panic!("expected source block");
        };
        let ExprKind::Match { arms, .. } = &block.value.result.value else {
            panic!("expected elaborated match");
        };
        assert_eq!(arms.len(), 1);
        assert!(arms[0].value.irrefutable_required);
        assert!(matches!(arms[0].value.pattern.value, PatternKind::Tuple(_)));
    }

    #[test]
    fn rejects_module_level_destructuring_let() {
        let error = parse("test.telora", "let (left, right) = (1, 2); left").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("destructuring let is allowed only inside a local block")
        );
    }

    #[test]
    fn lowers_explicit_exports_without_creating_lexical_bindings() {
        let program = parse(
            "exports.telora",
            r#"export let value = 1;
export def identity = fn(item) { item };
export @struct type User = { name: String };
let private = 2;
export { private as visible, identity as map };"#,
        )
        .unwrap();
        assert!(!program.value.authored_result);
        let bindings = &program.value.body.value.bindings;
        assert_eq!(
            bindings
                .iter()
                .filter(|binding| binding.value.kind == BindingKind::Export)
                .count(),
            5
        );
        assert_eq!(
            bindings
                .iter()
                .filter(|binding| binding.value.kind == BindingKind::Let)
                .count(),
            2
        );
        let visible = bindings
            .iter()
            .find(|binding| {
                binding.value.kind == BindingKind::Export && binding.value.name.value == "visible"
            })
            .unwrap();
        assert_eq!(
            visible.value.imported_name.as_deref().unwrap().value,
            "private"
        );
    }

    #[test]
    fn diagnoses_duplicate_mixed_and_nested_exports() {
        for (source, expected) in [
            (
                "export let value = 1; export { value };",
                "duplicate export",
            ),
            ("export let value = 1; value", "cannot mix explicit exports"),
            (
                "let value = { export let nested = 1; nested }; value",
                "only at module top level",
            ),
        ] {
            let error = parse("exports.telora", source).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn lowers_heterogeneous_tuple_contracts_through_array_metadata() {
        let program = parse(
            "test.telora",
            "native pairs: Fn(Any) -> Array(Tuple([String, Any])); 0",
        )
        .unwrap();
        let annotation = program.value.body.value.bindings[0]
            .value
            .annotation
            .as_ref()
            .expect("native annotation");
        let ExprKind::Call { arguments, .. } = &annotation.value else {
            panic!("expected Fn metadata call");
        };
        let ExprKind::Call { arguments, .. } = &arguments[1].value else {
            panic!("expected Array metadata call");
        };
        let ExprKind::Call { arguments, .. } = &arguments[0].value else {
            panic!("expected Tuple metadata call");
        };
        assert!(matches!(&arguments[0].value, ExprKind::Array(items) if items.len() == 2));
    }

    #[test]
    fn function_notation_lowers_to_the_func_metadata_constructor() {
        let program = parse("test.telora", "native convert: Fn(A) -> Tuple([B, C]); 0").unwrap();
        let annotation = program.value.body.value.bindings[0]
            .value
            .annotation
            .as_ref()
            .expect("native annotation");
        let ExprKind::Call { callee, arguments } = &annotation.value else {
            panic!("expected Func metadata call");
        };
        assert!(is_variable(callee, "Func"));
        assert!(matches!(&arguments[0].value, ExprKind::Array(items) if items.len() == 1));
        let ExprKind::Call { callee, arguments } = &arguments[1].value else {
            panic!("expected Tuple metadata call");
        };
        assert!(is_variable(callee, "Tuple"));
        assert!(matches!(&arguments[0].value, ExprKind::Array(items) if items.len() == 2));
    }

    #[test]
    fn rejects_constructor_shaped_fn_notation() {
        let error = parse("test.telora", "native invalid: Fn([A], B); 0").unwrap_err();
        assert!(error.to_string().contains("expected"), "{error}");
    }

    #[test]
    fn diagnoses_invalid_placeholder_sections_with_source_locations() {
        let cases = [
            (
                "mixed.telora",
                "let f = fn(a, b) { a }; f\\(_0, _)",
                "cannot mix",
            ),
            (
                "gap.telora",
                "let f = fn(a, b) { a }; f\\(_2, _0)",
                "missing _1",
            ),
            (
                "limit.telora",
                "let f = fn(a) { a }; f\\(_65535)",
                "exceeds the limit",
            ),
            (
                "overflow.telora",
                "let f = fn(a) { a }; f\\(_999999999999999999999999999999999)",
                "exceeds the supported range",
            ),
        ];
        for (name, source, expected) in cases {
            let mut sources = SourceDatabase::default();
            let id = sources.add(name, source);
            let parsed = parse_registered(&sources, id);
            assert!(parsed.program.is_none(), "{name} unexpectedly lowered");
            let rendered = parsed
                .diagnostics
                .iter()
                .map(|diagnostic| sources.render(diagnostic))
                .collect::<Vec<_>>()
                .join("\n");
            assert!(rendered.contains(expected), "{rendered}");
            assert!(rendered.contains(&format!("{name}:1:")), "{rendered}");
        }

        let mut sources = SourceDatabase::default();
        let id = sources.add("outside.telora", "let value = _; value");
        let parsed = parse_registered(&sources, id);
        assert!(parsed.program.is_none());
        assert!(!parsed.diagnostics.is_empty());

        for (name, source) in [
            ("ordinary-call.telora", "let f = fn(a, b) { a }; f(_, 1)"),
            ("reserved-name.telora", "let _0 = 1; _0"),
        ] {
            let mut sources = SourceDatabase::default();
            let id = sources.add(name, source);
            let parsed = parse_registered(&sources, id);
            assert!(parsed.program.is_none(), "{name} unexpectedly lowered");
            assert!(!parsed.diagnostics.is_empty(), "{name} has no diagnostic");
        }

        let mut sources = SourceDatabase::default();
        let id = sources.add("empty-section.telora", "let f = fn(a) { a }; f\\(1)");
        let parsed = parse_registered(&sources, id);
        assert!(parsed.program.is_none());
        assert!(
            parsed.diagnostics[0]
                .message
                .contains("requires at least one placeholder")
        );
    }

    #[test]
    fn lowers_only_direct_type_argument_placeholders() {
        let program = parse("types.telora", "pair[Int, _](1, \"x\")").unwrap();
        let ExprKind::Call { callee, .. } = &program.value.body.value.result.value else {
            panic!("expected call");
        };
        let ExprKind::TypeApply { arguments, .. } = &callee.value else {
            panic!("expected type application");
        };
        assert!(matches!(arguments[0].value, TypeArgumentKind::Explicit(_)));
        assert!(matches!(arguments[1].value, TypeArgumentKind::Infer));
        assert_eq!(arguments[1].location.range(), 10..11);

        for source in ["pair[Int, _0](1, 2)", "pair[Array(_), Int](1, 2)"] {
            let mut sources = SourceDatabase::default();
            let id = sources.add("invalid.telora", source);
            let parsed = parse_registered(&sources, id);
            assert!(parsed.program.is_none(), "{source} unexpectedly parsed");
            assert!(!parsed.diagnostics.is_empty(), "{source} has no diagnostic");
        }
    }

    #[test]
    fn exposes_all_recovery_diagnostics() {
        let mut sources = SourceDatabase::default();
        let id = sources.add("broken.telora", "let x = ; let y = ; y");
        let parsed = parse_registered(&sources, id);
        assert!(parsed.program.is_none());
        assert!(parsed.diagnostics.len() >= 2);
    }

    #[test]
    fn recovers_complete_bindings_around_a_damaged_sibling() {
        let mut sources = SourceDatabase::default();
        let id = sources.add(
            "recover.telora",
            "let before = 1; let broken = ; let after = 2; after",
        );
        let parsed = parse_registered(&sources, id);
        assert!(parsed.program.is_none());
        assert!(!parsed.diagnostics.is_empty());
        let names = parsed
            .recovered
            .bindings
            .iter()
            .map(|binding| binding.value.name.value.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["before", "after"]);
        assert!(parsed.recovered.result.is_some());
    }

    #[test]
    fn comparisons_share_a_non_associative_precedence_level() {
        let chained = parse("test", "1 < 2 == 3").unwrap_err();
        assert!(chained.message.contains("do not associate"));
        assert!(parse("test", "(1 < 2) == 3").is_ok());
        assert!(parse("test", "1 < (2 == 3)").is_ok());
    }

    #[test]
    fn lowers_interpolation_with_located_text_and_expression_parts() {
        let program = parse("test", r#"let name = "Ada"; `hi, \{name}`"#).unwrap();
        let ExprKind::InterpolatedString(parts) = &program.value.body.value.result.value else {
            panic!("expected interpolated string");
        };
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0].value, StringPartKind::Text(text) if text == "hi, "));
        assert!(matches!(
            &parts[1].value,
            StringPartKind::Expression(expression)
                if matches!(&expression.value, ExprKind::Variable(name) if name.value == "name")
        ));
        assert_eq!(parts[1].location.range(), 25..29);
    }

    #[test]
    fn lowers_definition_bindings_and_function_contracts() {
        let program = parse(
            "defs.telora",
            "decl f: Fn(Int) -> Int; def f = fn(x) { x }; f",
        )
        .unwrap();
        assert_eq!(program.value.body.value.bindings.len(), 2);
        assert_eq!(
            program.value.body.value.bindings[0].value.kind,
            BindingKind::Decl
        );
        assert_eq!(
            program.value.body.value.bindings[1].value.kind,
            BindingKind::Def
        );
        assert!(matches!(
            program.value.body.value.bindings[0]
                .value
                .annotation
                .as_ref()
                .map(|annotation| &annotation.value),
            Some(ExprKind::Call { .. })
        ));
    }

    #[test]
    fn lowers_generic_definition_declarations_with_located_parameters() {
        let program = parse(
            "identity.telora",
            "decl identity: for(A) Fn(A) -> A; def identity = fn(value) { value }; identity",
        )
        .unwrap();
        let declaration = &program.value.body.value.bindings[0];
        assert_eq!(declaration.value.kind, BindingKind::Decl);
        assert_eq!(declaration.value.type_parameters.len(), 1);
        assert_eq!(declaration.value.type_parameters[0].value, "A");
        assert_eq!(
            declaration.value.type_parameters[0].location.range(),
            19..20
        );
        assert!(declaration.value.annotation.is_some());
    }

    #[test]
    fn lowers_located_native_bindings_with_contracts() {
        let program = parse(
            "native.telora",
            "native map: for(A, B) Fn(Array(A), Fn(A) -> B) -> Array(B); map",
        )
        .unwrap();
        let binding = &program.value.body.value.bindings[0];
        assert_eq!(binding.value.kind, BindingKind::Native);
        assert_eq!(binding.value.name.value, "map");
        assert_eq!(
            binding
                .value
                .type_parameters
                .iter()
                .map(|parameter| parameter.value.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "B"]
        );
        assert_eq!(binding.value.type_parameters[0].location.range(), 16..17);
        assert!(binding.value.annotation.is_some());
        assert_eq!(binding.location.range(), 0..59);
    }

    #[test]
    fn lowers_native_type_declarations_with_explicit_slots() {
        let program = parse(
            "native-type.telora",
            "native type State @3; native new: Fn() -> State; State",
        )
        .unwrap();
        let binding = &program.value.body.value.bindings[0];
        assert_eq!(binding.value.kind, BindingKind::NativeType);
        assert_eq!(binding.value.name.value, "State");
        assert!(binding.value.annotation.is_none());
        assert!(matches!(binding.value.value.value, ExprKind::Int(3)));
        assert_eq!(binding.value.value.location.range(), 19..20);
        assert_eq!(binding.location.range(), 0..21);
    }

    #[test]
    fn retains_decorators_and_lowers_their_rhs_calls() {
        let program = parse(
            "decorators.telora",
            "@outer @factory(1) type T = Int; { @field value: 2 }",
        )
        .unwrap();
        let binding = &program.value.body.value.bindings[0];
        assert_eq!(binding.value.decorators.len(), 2);
        assert!(!binding.value.decorators[0].value.configured);
        assert!(binding.value.decorators[1].value.configured);
        assert!(matches!(binding.value.value.value, ExprKind::Call { .. }));
        let ExprKind::Dict(fields) = &program.value.body.value.result.value else {
            panic!("expected Dict")
        };
        assert_eq!(fields[0].value.decorators.len(), 1);
        assert!(matches!(fields[0].value.value.value, ExprKind::Call { .. }));
    }

    #[test]
    fn lowers_parameterized_type_declarations_with_located_parameters() {
        let program = parse(
            "family.telora",
            "@struct type Pair(Left, Right) = {left: Left, right: Right}; Pair",
        )
        .unwrap();
        let binding = &program.value.body.value.bindings[0];
        assert_eq!(binding.value.kind, BindingKind::Type);
        assert_eq!(binding.value.name.value, "Pair");
        assert_eq!(
            binding
                .value
                .type_parameters
                .iter()
                .map(|parameter| parameter.value.as_str())
                .collect::<Vec<_>>(),
            vec!["Left", "Right"]
        );
        assert_eq!(binding.value.type_parameters[0].location.range(), 18..22);
        assert_eq!(binding.value.type_parameters[1].location.range(), 24..29);
        assert!(matches!(binding.value.value.value, ExprKind::Call { .. }));
    }

    #[test]
    fn separates_strings_from_concat_only_contexts() {
        let string_error = parse("test", r#""\{1}""#).unwrap_err();
        assert!(string_error.message.contains("unsupported string escape"));
        assert!(parse("test", r#"match "x" { `\{1}` => 1 }"#).is_err());
        assert!(parse("test", r#"{`\{"x"}`: 1}"#).is_err());
    }

    #[test]
    fn reports_invalid_and_unterminated_string_parts() {
        let invalid = parse("test", r#""bad\q""#).unwrap_err();
        assert!(invalid.message.contains("unsupported string escape"));
        assert_eq!(invalid.location.offset, 4);

        let unterminated = parse("test", r#""unfinished"#).unwrap_err();
        assert!(unterminated.message.contains("expected"));

        let non_ascii = parse("test", r#""\xff""#).unwrap_err();
        assert!(non_ascii.message.contains("must be ASCII"));

        let invalid_scalar = parse("test", r#""\u{d800}""#).unwrap_err();
        assert!(invalid_scalar.message.contains("Unicode scalar"));
    }

    #[test]
    fn lowers_only_immediate_ordered_module_options() {
        let program = parse(
            "options.telora",
            r#"option "module.documentation" {name: "tool"}; export let value = 0; option "module.documentation" 'Stable;"#,
        )
        .unwrap();
        assert_eq!(program.value.options.len(), 2);
        assert_eq!(program.value.options[0].key.value, "module.documentation");
        assert!(matches!(
            program.value.options[0].value.value,
            ExprKind::Dict(_)
        ));
        assert!(matches!(
            program.value.options[1].value.value,
            ExprKind::Atom(_)
        ));

        for invalid in [
            "@@manifest {}; export let value = 0;",
            "option \"documentation\" {}; export let value = 0;",
            "option \"module.documentation\" value; export let value = 0;",
            "option \"module.documentation\" `tool-\\{value}`; export let value = 0;",
            "option \"module.documentation\" {...value}; export let value = 0;",
            "export def f = fn() { option \"module.documentation\" {}; 0 };",
        ] {
            assert!(
                parse("invalid-option.telora", invalid).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn selective_imports_are_in_scope_for_exported_definition_contracts() {
        let program = parse(
            "selective-type.telora",
            r#"import "std/rt-types/exec.telora" { ExecFn };
               export def exec: ExecFn = fn(settings, request) { request };"#,
        )
        .unwrap();
        assert_eq!(
            program
                .value
                .body
                .value
                .bindings
                .iter()
                .map(|binding| binding.value.name.value.as_str())
                .collect::<Vec<_>>(),
            vec!["ExecFn", "exec", "exec"]
        );
        let hir = crate::hir::HirProgram::resolve(&program, Vec::<String>::new());
        assert!(
            hir.unresolved().next().is_none(),
            "selective import must precede the exported contract"
        );
    }

    #[test]
    fn preserves_interpreter_operand_in_ast() {
        let program = parse(
            "interpreter.telora",
            "def lift: for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool = interpreter!(eq_i); lift",
        )
        .unwrap();
        let value = &program.value.body.value.bindings[0].value.value;
        let ExprKind::Interpreter { operand, .. } = &value.value else {
            panic!("expected interpreter expression")
        };
        assert!(matches!(
            &operand.value,
            ExprKind::Variable(name) if name.value == "eq_i"
        ));
    }

    #[test]
    fn parses_propagation_as_a_postfix_expression() {
        let program = parse("propagate.telora", "value?").unwrap();
        assert!(matches!(
            program.value.body.value.result.value,
            ExprKind::Propagate { .. }
        ));

        let program = parse("propagate.telora", "left + right?").unwrap();
        let ExprKind::Binary { right, .. } = &program.value.body.value.result.value else {
            panic!("expected binary expression")
        };
        assert!(matches!(right.value, ExprKind::Propagate { .. }));
    }

    #[test]
    fn diagnoses_contextual_intrinsic_names_and_arity() {
        for (source, expected) in [
            (
                "def lift: for(A) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter(eq_i); lift",
                "interpreter(...) has been replaced by interpreter!(...)",
            ),
            ("unknown!(1)", "unknown contextual intrinsic unknown!"),
            (
                "blame!()",
                "blame! expects a message followed by zero or more subjects",
            ),
            (
                "emit_warn!()",
                "emit_warn! expects a message followed by zero or more subjects",
            ),
            (
                "fail!()",
                "fail! expects a message followed by zero or more subjects",
            ),
            ("file!()", "file! is reserved but not implemented"),
            ("line!()", "line! is reserved but not implemented"),
            (
                "def lift: for(A) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter!(); lift",
                "interpreter! expects exactly one argument, found 0",
            ),
            (
                "def lift: for(A) Fn(TypeOf(A)) -> Fn(A) -> Bool = interpreter!(a, b); lift",
                "interpreter! expects exactly one argument, found 2",
            ),
        ] {
            let error = parse("intrinsic.telora", source).unwrap_err();
            assert!(
                error.message.contains(expected),
                "expected {expected:?}, got {:?}",
                error.message
            );
        }
    }

    #[test]
    fn lowers_blame_to_the_canonical_sourced_record() {
        let program = parse("blame.telora", "blame!(message, data)").unwrap();
        let ExprKind::Dict(fields) = &program.value.body.value.result.value else {
            panic!("expected blame record")
        };
        assert_eq!(
            fields
                .iter()
                .map(|field| {
                    field
                        .value
                        .name
                        .as_ref()
                        .expect("blame fields have names")
                        .value
                        .as_str()
                })
                .collect::<Vec<_>>(),
            ["data", "message", "rule"]
        );
        assert!(matches!(
            &fields[2].value.value.value,
            ExprKind::String(marker) if marker == "blame!"
        ));
        assert_eq!(fields[2].value.value.location.range(), 0..21);
    }
}
