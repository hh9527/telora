use super::*;

fn parse(text: &str) -> (Mir, HirId, CstData) {
    let mut mir = Mir::default();
    let source = mir.sources.add("test", text);
    let parsed = crate::syntax::telora::parse_document(source, mir.sources.get(source).text().document().expect("code source"));
    let lowered = lower_module(&mut mir, ModuleId(0), source, &parsed.syntax);
    mir.diagnostics = parsed.diagnostics;
    mir.diagnostics.extend(lowered.diagnostics);
    (mir, lowered.body, parsed.syntax)
}

#[test]
fn lowers_directly_from_cst_with_spans_and_precedence() {
    let (mir, body, _) = parse("let x = 1; x == 2");
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let body = &mir.hir[body.index()];
    assert_eq!(body.location.range(), 0..17);
    assert_eq!(
        mir.hir[body.children[0].node.index()].location.range(),
        0..10
    );
    let result = body
        .children
        .iter()
        .find(|edge| edge.role == Role::Result)
        .unwrap()
        .node;
    assert!(matches!(
        mir.hir[result.index()].kind,
        HirKind::Binary(crate::syntax::kinds::BinaryOperator::Equal)
    ));
}

#[test]
fn statement_bodies_keep_bindings_and_tail_syntax_visible() {
    use crate::syntax::telora::ast::{AstNode, Body};
    for (source, has_tail) in [
        ("do { 1; # first\n let a = 2; a; }", false),
        ("do { 1; let a = 2; a }", true),
    ] {
        let (mir, root, cst) = parse(source);
        assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
        let result = mir.hir[root.index()]
            .children
            .iter()
            .find(|edge| edge.role == Role::Result)
            .unwrap()
            .node;
        let block = &mir.hir[result.index()];
        assert!(matches!(block.kind, HirKind::Block));
        assert_eq!(
            block
                .children
                .iter()
                .filter(|edge| edge.role == Role::Binding)
                .count(),
            if has_tail { 2 } else { 3 }
        );
        let body = SyntaxNode::new(&cst, NodeRef::ROOT)
            .descendants_and_self()
            .find(|node| node.rule() == Some(Rule::Body))
            .unwrap();
        let body = Body::cast(&cst, body.node_ref()).unwrap();
        assert_eq!(body.bindings().count(), 1);
        assert_eq!(body.result().is_some(), has_tail);
    }
}

#[test]
fn malformed_else_if_chain_recovers_without_panicking() {
    let (mir, _, _) = parse("if True { 1 } else if { 2 } else { 3 }");
    assert!(!mir.diagnostics.is_empty());
}

#[test]
fn incomplete_operand_preserves_binary_structure() {
    let (mir, _, _) = parse("let a = x + ; let b = 123; b");
    assert!(!mir.diagnostics.is_empty());
    let binary = mir
        .hir
        .iter()
        .find(|node| matches!(node.kind, HirKind::Binary(_)))
        .expect("the operator remains meaningful when its right operand is missing");
    let left = binary
        .children
        .iter()
        .find(|edge| edge.role == Role::Left)
        .unwrap();
    let right = binary
        .children
        .iter()
        .find(|edge| edge.role == Role::Right)
        .unwrap();
    assert!(matches!(&mir.hir[left.node.index()].kind, HirKind::Variable(name) if name == "x"));
    assert!(matches!(mir.hir[right.node.index()].kind, HirKind::Missing));
    assert!(
        mir.hir
            .iter()
            .any(|node| matches!(node.kind, HirKind::Int(123)))
    );
}

#[test]
fn incomplete_field_preserves_receiver_reference() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/hir-lower/incomplete-field.telora"
    ))
    .unwrap();
    let (mir, _, _) = parse(&text);
    assert!(!mir.diagnostics.is_empty());
    let field = mir
        .hir
        .iter()
        .find(|node| matches!(node.kind, HirKind::Field))
        .unwrap();
    let receiver = field
        .children
        .iter()
        .find(|edge| edge.role == Role::Receiver)
        .unwrap();
    assert!(
        matches!(&mir.hir[receiver.node.index()].kind, HirKind::Variable(name) if name == "value")
    );
}

#[test]
fn incomplete_syntax_remains_safe_through_static_passes() {
    for fixture in [
        "incomplete-field",
        "incomplete-parameter",
        "incomplete-record",
        "incomplete-condition",
    ] {
        let text = std::fs::read_to_string(format!(
            "{}/../../tests/fixtures/hir-lower/{fixture}.telora",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let mut mir = crate::module_resolve::resolve(
            vec![crate::module_resolve::ModuleSpec {
                native: None,
                name: "main".into(),
                kind: ModuleKind::Source,
                implicit_imports: vec![],
            }],
            &["main".into()],
            |_, _| Ok(text.clone()),
        );
        assert!(!mir.diagnostics.is_empty(), "{fixture}");
        crate::symbol_resolve::resolve(&mut mir);
        crate::type_resolve::resolve(&mut mir);
        assert!(mir.seal().is_err(), "{fixture}");
    }
}

#[test]
fn preserves_independent_recovery_diagnostics() {
    let (mir, _, _) = parse("let x = ; let y = ; y");
    assert!(mir.diagnostics.len() >= 2);
}

#[test]
fn lowering_preserves_parser_diagnostics_without_reinterpreting_recovery() {
    let cases = [
        "export def broken = (1 + 2;",
        "export def broken = match A { A 1, _ => 2 };",
        "type Broken = enum { @bad(\"name\") }; export {Broken};",
        "type Broken = enum { @bad(\"name\", Bad }; export {Broken};",
        "export def broken = match A { @ => 1, _ => 2 };",
    ];
    for text in cases {
        let mut sources = crate::source::SourceDatabase::default();
        let source = sources.add("test", text);
        let parsed = crate::syntax::telora::parse(source, text);
        assert!(!parsed.diagnostics.is_empty());
        let expected = parsed
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        let (mir, _, _) = parse(text);
        assert_eq!(
            mir.diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{text}\n{}",
            mir.dump()
        );
    }
}

#[test]
fn keeps_separate_syntax_roots_independently_actionable() {
    let cases: &[(&str, &[&str])] = &[
        (
            "export def first = (1 + 2; export def second = match A { A 1, _ => 2 };",
            &[
                "invalid syntax, expected one of: ',', ')'",
                "invalid syntax, expected one of: '=>', 'if'",
            ],
        ),
        (
            "export def broken = match A { A 1, B 2, _ => 3 };",
            &[
                "invalid syntax, expected one of: '=>', 'if'",
                "invalid syntax, expected one of: '=>', 'if'",
            ],
        ),
    ];
    for (text, expected) in cases {
        let (mir, _, _) = parse(text);
        assert_eq!(
            mir.diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>(),
            *expected,
            "{text}"
        );
    }
}

#[test]
fn recovers_complete_bindings_around_a_damaged_sibling() {
    let (mir, root, _) = parse("let before = 1; let broken = ; let after = 2; after");
    assert!(!mir.diagnostics.is_empty());
    let body = &mir.hir[root.index()];
    let names = body
        .children
        .iter()
        .filter(|edge| edge.role == Role::Binding)
        .map(|edge| {
            let binding = &mir.hir[edge.node.index()];
            let name = binding
                .children
                .iter()
                .find(|edge| edge.role == Role::Name)
                .unwrap()
                .node;
            let HirKind::Name(name) = &mir.hir[name.index()].kind else {
                panic!("name")
            };
            name.as_str()
        })
        .collect::<Vec<_>>();
    assert_eq!(names, ["before", "broken", "after"]);
    assert!(body.children.iter().any(|edge| edge.role == Role::Result));
}

#[test]
fn reports_invalid_and_unterminated_string_parts() {
    let error = |text| parse(text).0.diagnostics.into_iter().next().unwrap();
    let invalid = error(r#""bad\q""#);
    assert!(invalid.message.contains("unsupported string escape"));
    assert_eq!(invalid.labels[0].location.start, 4);
    assert!(error(r#""unfinished"#).message.contains("expected"));
    assert!(error(r#""\xff""#).message.contains("must be ASCII"));
    assert!(error(r#""\u{d800}""#).message.contains("Unicode scalar"));
}
