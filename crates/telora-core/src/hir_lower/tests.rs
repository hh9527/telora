use super::*;
use crate::mir::{Module, ModuleKind, ModuleState, ResolveState, TypeState};

fn parsed(text: &str) -> (Mir, SourceId, CstData, NodeRef) {
    let mut mir = Mir::default();
    let source = mir.sources.add("test", text);
    let parsed = crate::syntax::telora::parse_document(source, mir.sources.get(source).text());
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let program = crate::syntax::telora::ast::Program::root(&parsed.syntax);
    let root = program
        .body()
        .unwrap()
        .result()
        .unwrap()
        .syntax()
        .node_ref();
    mir.modules.push(Module {
        native: None,
        name: "test".into(),
        kind: ModuleKind::Source,
        state: ModuleState::Unloaded,
        imports: vec![],
    });
    (mir, source, parsed.syntax, root)
}

fn lower(text: &str) -> (Mir, HirId) {
    let (mut mir, source, cst, root) = parsed(text);
    let result = expression(&mut mir, ModuleId(0), source, &cst, root).unwrap();
    mir.modules[0].state = ModuleState::Source {
        source,
        syntax_valid: true,
        cst,
        body: result,
    };
    (mir, result)
}

#[test]
fn scopes_have_stable_edges_slots_and_syntax_origins() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/hir-lower/scopes.telora");
    let source = std::fs::read_to_string(path).unwrap();
    let (mir, root) = lower(&source);
    let (again, same_root) = lower(&source);
    assert_eq!(root, same_root);
    assert_eq!(mir.dump(), again.dump());
    assert_eq!(mir.hir.len(), mir.ty_slots.len());
    assert!(mir.hir.iter().any(|node| matches!(
        node.kind,
        HirKind::TypeOperation(crate::mir::TypeOperation::Function)
    )));
    assert!(
        mir.ty_slots
            .iter()
            .all(|ty| matches!(ty, TypeState::Unknown))
    );
    assert!(
        mir.resolve_slots
            .iter()
            .all(|slot| matches!(slot, ResolveState::Pending))
    );
    let ModuleState::Source { cst, .. } = &mir.modules[0].state else {
        unreachable!()
    };
    for (index, node) in mir.hir.iter().enumerate() {
        let syntax = match node.origin.unwrap() {
            HirOrigin::Source(node) | HirOrigin::Desugared(node) => node,
        };
        assert_eq!(
            node.location,
            Location::from_usize(node.location.source, cst.span(syntax)).unwrap()
        );
        assert!(node.children.iter().all(|edge| edge.node.index() < index));
    }
    let block = &mir.hir[root.index()];
    assert!(matches!(block.kind, HirKind::Block));
    assert_eq!(
        block
            .children
            .iter()
            .filter(|edge| edge.role == Role::Binding)
            .count(),
        3
    );
    let pipeline = mir
        .hir
        .iter()
        .find(|node| {
            matches!(node.kind, HirKind::Call)
                && matches!(node.origin, Some(HirOrigin::Desugared(_)))
        })
        .expect("pipeline expands to a call");
    assert_eq!(
        pipeline
            .children
            .iter()
            .map(|edge| edge.role)
            .collect::<Vec<_>>(),
        [Role::Callee, Role::Argument]
    );
    let closure = mir
        .hir
        .iter()
        .find(|node| matches!(node.kind, HirKind::Closure))
        .unwrap();
    assert_eq!(
        closure
            .children
            .iter()
            .map(|edge| edge.role)
            .collect::<Vec<_>>(),
        [Role::Parameter, Role::ReturnType, Role::Body]
    );
}

#[test]
fn deep_expression_lowering_and_drop_do_not_use_the_native_call_stack() {
    // Parse outside the small-stack thread: this test isolates lowering and
    // destruction, and makes no claim about the generated recursive parser.
    let source = std::iter::repeat_n("1", 4000)
        .collect::<Vec<_>>()
        .join(" + ");
    let (mut mir, source, cst, root) = parsed(&source);
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(move || {
            let result = expression(&mut mir, ModuleId(0), source, &cst, root).unwrap();
            assert!(matches!(mir.hir[result.index()].kind, HirKind::Binary(_)));
            assert_eq!(mir.hir.len(), 7999);
            drop(mir);
            drop(cst);
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn unsupported_rules_are_explicit_and_do_not_invoke_the_owned_ast() {
    let (mut mir, source, cst, root) = parsed("match 1 { _ => 2 }");
    let diagnostic = expression(&mut mir, ModuleId(0), source, &cst, root).unwrap_err();
    assert!(diagnostic.message.contains("not implemented"));
    assert!(mir.hir.is_empty());
}
