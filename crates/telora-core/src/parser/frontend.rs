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
                | Rule::TraitMember
                | Rule::ImplMember
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
                    | Rule::TraitBinding
                    | Rule::ImplBinding
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
