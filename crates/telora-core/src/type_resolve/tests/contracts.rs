use super::*;

#[test]
fn contract_checks_cover_value_shapes_and_unresolved_aliases() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/language/src/check");
    for (fixture, names, unresolved) in [
        ("diag-top-level-contract", &["scalar", "container", "metadata", "function", "exported", "renamed"][..], false),
        ("diag-top-level-contract-unresolved", &["aliased"][..], true),
    ] {
        let source = std::fs::read_to_string(directory.join(fixture).join("testee.telora")).unwrap();
        let mut mir = graph(&[("@src/main", &source)]);
        resolve(&mut mir);
        for name in names {
            let contract = mir.declaration_contracts.iter().find(|contract|
                mir.symbols[contract.symbol.index()].name == *name).unwrap();
            assert!(if unresolved { contract.state == DeclarationContractState::Unresolved }
                else { contract.state == DeclarationContractState::Missing }, "{name}: {contract:?}");
        }
        mir.diagnostics.clear();
        assert!(mir.seal().is_err());
    }
}

#[test]
fn explicit_error_paths_cannot_poison_closed_contracts() {
    let source = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/contract-error-boundaries.telora")).unwrap();
    let mut mir = graph(&[("@src/main", &source)]);
    resolve(&mut mir);
    for name in ["require_int", "metadata", "valid_witness", "number", "healthy_number",
        "consume", "broken", "propagated", "healthy_call"] {
        assert!(matches!(symbol_type(&mir, name), TypeState::Known(_)), "{name}: {}", mir.dump());
    }
    assert_eq!(mir.type_conflicts.len(), 3, "{:?}", mir.diagnostics);
    mir.diagnostics.clear();
    assert!(mir.seal().is_err());
}

#[test]
fn materialized_template_value_is_not_generalized_at_each_use() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let source = std::fs::read_to_string(directory.join("template-value-binding.telora")).unwrap();
    let library = std::fs::read_to_string(directory.join("template-library.telora")).unwrap();
    let mut mir = graph(&[("@src/main", &source), ("@src/library", &library)]);
    resolve(&mut mir);
    assert!(!mir.type_conflicts.is_empty(), "a function value cannot select different type arguments at each call: {}", mir.dump());
    let valid = mir.symbols.iter().find(|symbol| symbol.name == "valid").unwrap();
    assert!(valid.declarations.iter().all(|declaration| mir.type_conflicts_in(*declaration).is_empty()), "{:?}", mir.diagnostics);
    assert!(mir.seal().is_err());
}

#[test]
fn template_references_instantiate_independently_and_share_only_closed_instances() {
    let source = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/template-materialization.telora")).unwrap();
    let library = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/template-library.telora")).unwrap();
    let mut mir = graph(&[("@src/main", &source), ("@src/library", &library)]);
    resolve(&mut mir);
    mir.seal().unwrap_or_else(|d| panic!("{d:?}"));
    let references = mir.hir.iter().enumerate().filter(|(_, node)|
        mir.modules[node.module.index()].name == "@src/main" && matches!(&node.kind,
            HirKind::Variable(name) if name == "foo")).map(|(index, _)| {
        let Some(GenericReference::Instance(instance)) = mir.generic_references[index] else {
            panic!("foo reference must select an instance");
        };
        instance
    }).collect::<Vec<_>>();
    assert_eq!(references.len(), 3);
    let instances = references.iter().copied().collect::<BTreeSet<_>>();
    assert_eq!(instances.len(), 2, "the two String uses can share code; the Int use cannot");
    let arguments = instances.iter().map(|id| {
        let instance = &mir.generic_instances[id.index()];
        assert!(instance.concrete);
        mir.types[instance.arguments[0].1.index()].constructor.clone()
    }).collect::<BTreeSet<_>>();
    assert_eq!(arguments, BTreeSet::from([TypeConstructor::Int, TypeConstructor::String]));
}

#[test]
fn standard_library_top_level_contracts_are_explicit() {
    let sources = crate::static_sources::BUILTINS;
    let inventory = sources.iter().map(|(name, _)| ModuleSpec {
        native: crate::static_sources::native_module(name),
        name: (*name).into(), kind: ModuleKind::Source,
        implicit_imports: if *name == "std/prelude" { vec![] } else { vec!["std/prelude".into()] },
    }).collect();
    let roots = sources.iter().map(|(name, _)| (*name).into()).collect::<Vec<_>>();
    let mut mir = module_resolve::resolve(inventory, &roots, |_, name| {
        Ok(sources.iter().find(|(key, _)| *key == name).unwrap().1.into())
    });
    crate::symbol_resolve::resolve(&mut mir);
    resolve(&mut mir);
    assert!(mir.declaration_contracts.iter().all(|contract|
        matches!(contract.state, DeclarationContractState::Complete(_))), "{}", mir.dump());
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    mir.seal().unwrap();
}

#[test]
fn explicit_export_obligations_survive_inferred_types_and_diagnostic_removal() {
    use crate::mir::DeclarationContractState;
    let source = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/export-contracts.telora")).unwrap();
    let mut mir = graph(&[("@src/main", &source)]);
    resolve(&mut mir);
    let state = |name: &str| &mir.declaration_contracts.iter().find(|contract|
        mir.symbols[contract.symbol.index()].name == name).unwrap().state;
    assert!(matches!(state("complete"), DeclarationContractState::Complete(_)));
    assert!(matches!(state("identity"), DeclarationContractState::Complete(_)));
    for name in ["missing", "alias", "private_inferred"] {
        assert!(matches!(state(name), DeclarationContractState::Missing));
        assert!(matches!(symbol_type(&mir, name), TypeState::Known(_)));
        let contract = mir.declaration_contracts.iter().find(|contract|
            mir.symbols[contract.symbol.index()].name == name).unwrap();
        assert!(!mir.declaration_contract_ready[contract.symbol.index()],
            "initializer evidence must not establish {name}'s contract");
    }
    for name in ["paired", "local_inference"] {
        assert!(matches!(state(name), DeclarationContractState::Complete(_)));
    }
    let TypeState::Known(ty) = symbol_type(&mir, "private_inferred") else { panic!("{}", mir.dump()); };
    assert_eq!(mir.types[ty.index()].constructor, TypeConstructor::String);
    mir.diagnostics.clear();
    assert!(mir.seal().is_err());
    mir.declaration_contracts.clear();
    assert!(mir.seal().is_err());
}

#[test]
fn failed_generic_use_and_body_keep_other_inference_results() {
    let source = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/language/src/check/diag-contract-local/testee.telora")).unwrap();
    let mut mir = graph(&[("@src/main", &source)]);
    resolve(&mut mir);
    for (name, expected) in [("good", TypeConstructor::Int), ("input", TypeConstructor::String),
        ("inferred", TypeConstructor::String)] {
        let TypeState::Known(ty) = symbol_type(&mir, name) else { panic!("{name}: {}", mir.dump()); };
        assert_eq!(mir.types[ty.index()].constructor, expected);
    }
    let TypeState::Known(signature) = symbol_type(&mir, "broken_body") else { panic!("{}", mir.dump()); };
    assert_eq!(mir.types[signature.index()].constructor, TypeConstructor::Function);
    assert!(mir.types[signature.index()].arguments.iter().all(|ty| mir.types[ty.index()].constructor == TypeConstructor::Int));
    assert_eq!(mir.type_conflicts.len(), 2, "{:?}", mir.diagnostics);
    for name in ["same", "broken_body"] {
        let symbol = mir.symbols.iter().find(|symbol| symbol.name == name).unwrap();
        let annotation = symbol.declarations.iter().find_map(|node|
            mir.hir[node.index()].children.iter().find(|edge| edge.role == Role::Annotation)
                .map(|edge| edge.node)).unwrap();
        assert!(mir.type_conflicts.iter().any(|failure| failure.contracts.contains(&annotation)),
            "{name}: {:?}", mir.type_conflicts);
    }
    assert!(mir.seal().is_err());
}

#[test]
fn rejected_uses_preserve_shared_contracts_and_independent_diagnostics() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/language/src/check/diag-shared-contract");
    for (root, reverse) in [("testee", false), ("reversed", true)] {
        let mut sources = [root, "shared", "good", "bad"].map(|name| {
            let file = if reverse && name == "bad" { "bad-reversed" } else { name };
            (format!("@src/check/diag-shared-contract/{name}"),
                std::fs::read_to_string(directory.join(format!("{file}.telora"))).unwrap())
        });
        if reverse { sources[1..].reverse(); }
        let inventory = sources.iter().map(|(name, source)| (name.as_str(), source.as_str())).collect::<Vec<_>>();
        let mut mir = graph(&inventory);
        resolve(&mut mir);
        let known = |name| match symbol_type(&mir, name) {
            TypeState::Known(ty) => &mir.types[ty.index()],
            state => panic!("{root}: {name}: {state:?}"),
        };
        assert_eq!(known("answer").constructor, TypeConstructor::Int);
        assert_eq!(known("result").constructor, TypeConstructor::Int);
        assert_eq!(known("input").constructor, TypeConstructor::String);
        let signature = known("increment");
        assert_eq!(signature.constructor, TypeConstructor::Function);
        assert!(signature.arguments.iter().all(|ty| mir.types[ty.index()].constructor == TypeConstructor::Int));
        assert_eq!(mir.type_conflicts.len(), 2, "{:?}", mir.diagnostics);
        assert_eq!(mir.diagnostics.len(), 2, "{:?}", mir.diagnostics);
        assert!(mir.diagnostics.iter().all(|d| d.labels.iter().any(|label|
            label.primary && mir.sources.get(label.location.source).name.ends_with("/bad"))));
        assert!(mir.diagnostics.iter().all(|d| d.labels.iter().any(|label|
            !label.primary && label.message == "type contract declared here"
                && mir.sources.get(label.location.source).name.ends_with("/shared"))), "{:?}", mir.diagnostics);
        assert!(mir.type_conflicts.iter().all(|failure| !failure.contracts.is_empty()));
        assert!(mir.seal().is_err());
        // Publication must be blocked by failed constraints, not only text.
        mir.diagnostics.clear();
        assert!(mir.seal().is_err());
    }
}
