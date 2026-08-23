use crate::ast::{
    BinaryOperator, Binding, BindingData, BindingKind, Block, BlockKind, ClosureParameter,
    DeclaredInitializerKind, Decorator, DecoratorKind, DictFieldKind, Expr, ExprKind, Identifier,
    MatchArm, MatchArmKind, OptionAction, Pattern, PatternKind, Program, ProgramKind,
    StringPartKind, StructPatternField, TypeArgument, TypeArgumentKind, UnaryOperator, located,
};
use crate::lexer::{FrontendError, SourceLocation};
use crate::source::{Diagnostic, Location, SourceDatabase, SourceId};
use crate::syntax::telora::lexer::Token;
use crate::syntax::telora::parser::{CstData, Node, NodeRef, Rule};
use std::collections::{BTreeMap, HashMap};

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
    let syntax_diagnostics = parsed.diagnostics;
    let mut lowering_diagnostics = Vec::new();
    let lowerer = Lowerer::new(source_id, source.text(), &parsed.syntax);
    let options = match lowerer.option_actions() {
        Ok(options) => options,
        Err(diagnostic) => {
            push_unique_diagnostic(&mut lowering_diagnostics, diagnostic);
            Vec::new()
        }
    };
    let recovered = lowerer.recover_program(&mut lowering_diagnostics);
    let mut diagnostics = reconcile_frontend_diagnostics(
        source.text(),
        &parsed.syntax,
        syntax_diagnostics,
        lowering_diagnostics,
    );
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

#[derive(Clone, Copy)]
struct RecoveryUnit {
    node: NodeRef,
    start: usize,
    end: usize,
    accepts_trailing_diagnostic: bool,
}

#[derive(Clone, Copy)]
enum RecoveryCandidateKind {
    Parser,
    MissingToken(&'static str),
}

struct RecoveryCandidate {
    diagnostic: Diagnostic,
    kind: RecoveryCandidateKind,
}

fn reconcile_frontend_diagnostics(
    source: &crate::document::DocumentText,
    cst: &CstData,
    syntax: Vec<Diagnostic>,
    lowering: Vec<Diagnostic>,
) -> Vec<Diagnostic> {
    // Recovery keeps the CST useful, but public diagnostics commit to one root per grammar unit.
    let mut units = Vec::new();
    collect_recovery_units(cst, NodeRef::ROOT, &mut units);
    let mut selected = Vec::new();
    let mut grouped = BTreeMap::<NodeRef, Vec<RecoveryCandidate>>::new();

    for diagnostic in syntax.into_iter().chain(lowering) {
        let kind = if parser_expected_symbols(&diagnostic.message).is_some() {
            Some(RecoveryCandidateKind::Parser)
        } else {
            missing_token_symbol(&diagnostic.message).map(RecoveryCandidateKind::MissingToken)
        };
        let Some(kind) = kind else {
            selected.push((diagnostic, None));
            continue;
        };
        let unit = diagnostic
            .labels
            .first()
            .and_then(|label| recovery_unit_for(source, &units, label.location.range()));
        let candidate = RecoveryCandidate { diagnostic, kind };
        if let Some(unit) = unit {
            grouped.entry(unit).or_default().push(candidate);
        } else {
            selected.push((candidate.diagnostic, Some(candidate.kind)));
        }
    }

    for mut candidates in grouped.into_values() {
        candidates.sort_by_key(|candidate| diagnostic_start(&candidate.diagnostic));
        let specialized = candidates
            .iter()
            .enumerate()
            .find_map(|(missing_index, missing)| {
                let RecoveryCandidateKind::MissingToken(symbol) = missing.kind else {
                    return None;
                };
                let matching_parser = candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, candidate)| {
                        matches!(candidate.kind, RecoveryCandidateKind::Parser)
                            && parser_expects_symbol(&candidate.diagnostic.message, symbol)
                    })
                    .min_by_key(|(_, candidate)| diagnostic_start(&candidate.diagnostic));
                let (_, parser) = matching_parser?;
                let parser_start = diagnostic_start(&parser.diagnostic);
                let has_prior_root = candidates.iter().any(|candidate| {
                    matches!(candidate.kind, RecoveryCandidateKind::Parser)
                        && diagnostic_start(&candidate.diagnostic) < parser_start
                        && !parser_expects_symbol(&candidate.diagnostic.message, symbol)
                });
                (!has_prior_root).then_some(missing_index)
            });
        let chosen = if let Some(missing) = specialized {
            missing
        } else {
            candidates
                .iter()
                .position(|candidate| matches!(candidate.kind, RecoveryCandidateKind::Parser))
                .unwrap_or(0)
        };
        let candidate = candidates.swap_remove(chosen);
        selected.push((candidate.diagnostic, Some(candidate.kind)));
    }

    selected.sort_by_key(|(diagnostic, _)| diagnostic_start(diagnostic));
    let mut saw_syntax_root = false;
    selected.retain(|(diagnostic, kind)| {
        let parser_eof_fallout = matches!(kind, Some(RecoveryCandidateKind::Parser))
            && parser_expects_only(diagnostic, "<end of file>")
            && saw_syntax_root;
        if !parser_eof_fallout && kind.is_some() {
            saw_syntax_root = true;
        }
        !parser_eof_fallout
    });
    selected
        .into_iter()
        .map(|(diagnostic, _)| diagnostic)
        .collect()
}

fn collect_recovery_units(cst: &CstData, node: NodeRef, units: &mut Vec<RecoveryUnit>) {
    if let Node::Rule(rule, _) = cst.get(node) {
        let accepts_trailing_diagnostic = matches!(
            rule,
            Rule::Argument
                | Rule::ArrayItem
                | Rule::ContractArgument
                | Rule::DictItem
                | Rule::EnumInitializerVariant
                | Rule::ExportItem
                | Rule::ImportItem
                | Rule::MatchArm
                | Rule::Parameter
                | Rule::StructInitializerField
                | Rule::StructPatternField
        );
        let is_unit = accepts_trailing_diagnostic
            || matches!(
                rule,
                Rule::DeclBinding
                    | Rule::DefBinding
                    | Rule::ExportStatement
                    | Rule::ImportBinding
                    | Rule::LetBinding
                    | Rule::LetElseBinding
                    | Rule::LetPatternBinding
                    | Rule::NativeBinding
                    | Rule::NativeTypeBinding
                    | Rule::OptionBinding
                    | Rule::TypeBinding
            );
        if is_unit {
            let range = cst.span(node);
            units.push(RecoveryUnit {
                node,
                start: range.start,
                end: range.end,
                accepts_trailing_diagnostic,
            });
        }
    }
    for child in cst.children(node) {
        collect_recovery_units(cst, child, units);
    }
}

fn recovery_unit_for(
    source: &crate::document::DocumentText,
    units: &[RecoveryUnit],
    diagnostic: std::ops::Range<usize>,
) -> Option<NodeRef> {
    units
        .iter()
        .filter(|unit| {
            let contains = unit.start <= diagnostic.start && diagnostic.start < unit.end;
            // Inserted tokens are often reported immediately after the incomplete CST node.
            let trailing = unit.accepts_trailing_diagnostic
                && unit.end <= diagnostic.start
                && source
                    .slice(
                        crate::source::TextRange::from_usize(unit.end..diagnostic.start)
                            .expect("CST and diagnostic offsets fit source ranges"),
                    )
                    .is_ok_and(|text| text.chars().all(char::is_whitespace));
            contains || trailing
        })
        .min_by_key(|unit| unit.end.saturating_sub(unit.start))
        .map(|unit| unit.node)
}

fn diagnostic_start(diagnostic: &Diagnostic) -> u32 {
    diagnostic
        .labels
        .first()
        .map_or(u32::MAX, |label| label.location.start)
}

fn parser_expected_symbols(message: &str) -> Option<Vec<&str>> {
    // Lelwel exposes expected tokens through this stable diagnostic format, not structured data.
    let mut rest = message
        .strip_prefix("invalid syntax, expected one of: ")
        .or_else(|| message.strip_prefix("invalid syntax, expected: "))?;
    let mut symbols = Vec::new();
    while !rest.is_empty() {
        let end = match rest.as_bytes()[0] {
            b'\'' => rest[1..].find('\'').map(|index| index + 2),
            b'<' => rest.find('>').map(|index| index + 1),
            _ => None,
        }?;
        symbols.push(&rest[..end]);
        rest = &rest[end..];
        if rest.is_empty() {
            break;
        }
        rest = rest.strip_prefix(", ")?;
    }
    Some(symbols)
}

fn parser_expects_symbol(message: &str, symbol: &str) -> bool {
    parser_expected_symbols(message).is_some_and(|symbols| symbols.contains(&symbol))
}

fn parser_expects_only(diagnostic: &Diagnostic, symbol: &str) -> bool {
    parser_expected_symbols(&diagnostic.message)
        .is_some_and(|symbols| symbols.as_slice() == [symbol])
}

fn missing_token_symbol(message: &str) -> Option<&'static str> {
    Some(match message.strip_prefix("missing ")? {
        "Atom" => "<atom>",
        "Bang" => "'!'",
        "Bytes" => "<bytes>",
        "Colon" => "':'",
        "Else" => "'else'",
        "Equal" => "'='",
        "EqualEqual" => "'=='",
        "FatArrow" => "'=>'",
        "Float" => "<float>",
        "Identifier" => "<identifier>",
        "Int" => "<integer>",
        "LBrace" => "'{'",
        "LBracket" => "'['",
        "LParen" => "'('",
        "RBrace" => "'}'",
        "RBracket" => "']'",
        "RParen" => "')'",
        "Semicolon" => "';'",
        _ => return None,
    })
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

fn parse_float_literal(text: &str) -> Result<f64, &'static str> {
    let value = text.parse::<f64>().map_err(|_| "invalid Float literal")?;
    value
        .is_finite()
        .then_some(value)
        .ok_or("Float literal must be finite")
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
                        declared_initializer: None,
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
                        declared_initializer: None,
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
                        declared_initializer: None,
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
                        declared_initializer: None,
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
                        declared_initializer: None,
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
                let mut decorators = self.decorators(node)?;
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
                    decorators.push(model);
                    value
                } else {
                    self.expression(
                        self.children(node)
                            .find(|child| {
                                self.is_expression(*child) && self.cst.span(*child).start > start
                            })
                            .ok_or_else(|| self.error(node, "type has no value"))?,
                    )?
                };
                let value =
                    self.apply_decorators(&decorators, "Type", &name, value, self.location(node));
                Ok(located(
                    BindingData {
                        decorators,
                        kind: BindingKind::Type,
                        declared_initializer,
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
                        declared_initializer: None,
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
                    declared_initializer: None,
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
                    declared_initializer: None,
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
                            declared_initializer: None,
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
                    let value = self.apply_decorators(
                        &decorators,
                        "Field",
                        &name,
                        self.expression(value_node)?,
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
                    let value = self.apply_decorators(
                        &decorators,
                        "Field",
                        &name,
                        payload,
                        self.location(variant),
                    );
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
                    | Rule::DoExpr
                    | Rule::IndexExpr
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
                    | Rule::DotPostfixExpr
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
#[path = "parser/tests/mod.rs"]
mod tests;
