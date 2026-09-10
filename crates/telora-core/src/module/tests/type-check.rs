#[test]
fn types_only_checks_imports_and_never_executes_module_values() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(
        directory.join("src/main.telora"),
        "import \"./dependency\" { value }; export def result = value + 1;",
    )
    .unwrap();
    fs::write(
        directory.join("src/dependency.telora"),
        "export def value = 1 / 0;",
    )
    .unwrap();
    let resolver = session_workspace_resolver(&directory, &["@src/main", "@src/dependency"]);
    let snapshot = recovery_engine()
        .check_types_with_resolver(resolver)
        .unwrap();
    assert!(
        snapshot.diagnostics().is_empty(),
        "{:?}",
        snapshot.diagnostics()
    );
    fs::write(
        directory.join("src/dependency.telora"),
        "export def value: Int = \"bad\";",
    )
    .unwrap();
    let resolver = session_workspace_resolver(&directory, &["@src/main", "@src/dependency"]);
    let snapshot = recovery_engine()
        .check_types_with_resolver(resolver)
        .unwrap();
    assert!(
        snapshot
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Int")),
        "{:?}",
        snapshot.diagnostics()
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn types_only_checks_tool_code_without_running_providers_or_checks() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    for (source, valid) in [
        (
            r#"@property(PropertyTarget.Type) type Tag = struct { n: Int };
            def tag: Fn(Type, Option(Tag)) -> Tag = fn(target, previous) { fail!("must not execute") };
            @tag type Item = struct { value: Int }; export {Item};"#,
            true,
        ),
        (
            r#"@property(PropertyTarget.Type) type Tag = struct { n: Int };
            def tag: Fn(Type, Option(Tag)) -> Tag = fn(target, previous) { "wrong" };
            @tag type Item = struct { value: Int }; export {Item};"#,
            false,
        ),
        (
            r#"@check(fn(value) { fail!("must not execute") }) type Item = struct(Int);
            export def item = Item(1);"#,
            true,
        ),
        (
            r#"@check(fn(value) { "wrong" }) type Item = struct(Int); export {Item};"#,
            false,
        ),
    ] {
        fs::write(directory.join("src/main.telora"), source).unwrap();
        let resolver = session_workspace_resolver(&directory, &["@src/main"]);
        let snapshot = recovery_engine()
            .check_types_with_resolver(resolver)
            .unwrap();
        assert_eq!(
            snapshot.diagnostics().is_empty(),
            valid,
            "{source}: {:?}",
            snapshot.diagnostics()
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn types_only_preserves_imported_constructors_without_reading_data() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(directory.join("src/main.telora"),
        "import \"./types\" { Item }; import \"./data.json\" { data }; export def item = Item(1); export {data};").unwrap();
    fs::write(
        directory.join("src/types.telora"),
        "export type Item = struct(Int);",
    )
    .unwrap();
    for data in [b"{\"value\":1}".as_slice(), b"{", b"\xff"] {
        fs::write(directory.join("src/data.json"), data).unwrap();
        let resolver =
            session_workspace_resolver(&directory, &["@src/main", "@src/types", "@src/data.json"]);
        let snapshot = recovery_engine()
            .check_types_with_resolver(resolver)
            .unwrap();
        assert!(
            snapshot.diagnostics().is_empty(),
            "{:?}",
            snapshot.diagnostics()
        );
        let data_module = snapshot.modules().iter().find(|module|
            module.kind == WorkspaceModuleKind::Json).unwrap();
        assert_eq!(data_module.state, WorkspaceModuleState::Available);
        assert!(data_module.source.is_none(), "data contents must not enter static sources");
        assert!(data_module.export_schemes["data"].ends_with("Value"), "{:?}", data_module.export_schemes);
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn session_hir_resolves_reexported_members_before_dependency_type_solving() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(directory.join("src/model.telora"), r#"
        export type Choice = enum { done, pending };
        export Choice.{done as End};
        export def broken: Int = "not an Int";
    "#).unwrap();
    fs::write(directory.join("src/bridge.telora"), r#"
        import "./model" { Choice, End as Renamed };
        export { Choice, Renamed };
    "#).unwrap();
    fs::write(directory.join("src/main.telora"), r#"
        import "./bridge" { Choice, Renamed as Done, Renamed as Other };
        export def inspect: Fn(Choice) -> Int = fn(value) {
            match value { Done => 1, Choice.pending => 0 }
        };
        export def shadow = fn() {
            let Done = 0;
            match 1 { Done => Done }
        };
    "#).unwrap();
    let resolver = session_workspace_resolver(&directory,
        &["@src/main", "@src/model", "@src/bridge"]);
    let root = resolver.selected_root().unwrap();
    let mut sources = SourceDatabase::default();
    let graph = ModuleGraph::discover(&resolver, vec![root.clone()], &BTreeMap::new(),
        builtin_list().into_iter().map(|(name, _)| ModuleCName::builtin(name)),
        None, false, &mut sources).unwrap();
    let mut names = StaticNames::new(&graph);
    let resolution = names.module_resolution(graph.id(&root.id).unwrap()).unwrap();
    assert_eq!(resolution.imports["Done"], resolution.imports["Other"],
        "aliases must connect to the same export row before typing");
    let hir = resolution.hir;
    assert_eq!(hir.references().iter().filter(|reference|
        reference.name == "Done" && hir.is_member_pattern(reference.location)).count(), 1);
    assert_eq!(hir.definitions().iter().filter(|definition|
        definition.name == "Done" && definition.kind == crate::hir::HirDefinitionKind::Pattern).count(), 1);
    assert!(hir.unresolved().next().is_none(), "{:?}", hir.unresolved().collect::<Vec<_>>());
    // HIR exists despite a dependency type error; the full static pass must
    // still diagnose that error rather than treating name resolution as typing.
    let snapshot = recovery_engine().check_types_with_resolver(resolver.clone()).unwrap();
    assert!(snapshot.diagnostics().iter().any(|diagnostic| diagnostic.message.contains("Int")),
        "{:?}", snapshot.diagnostics());
    fs::write(directory.join("src/model.telora"),
        "export type Choice = enum { done, pending }; export Choice.{done as End};").unwrap();
    let snapshot = recovery_engine().check_types_with_resolver(resolver).unwrap();
    assert!(snapshot.diagnostics().is_empty(), "{:?}", snapshot.diagnostics());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn open_imports_are_search_scopes_and_only_references_create_inputs() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(directory.join("src/api.telora"), r#"
        export def used = 1; export def unused = 2; export def binder = 3;
        export type Choice = enum { pick, skip }; export Choice.{pick, skip};
    "#).unwrap();
    fs::write(directory.join("src/main.telora"), r#"
        import "./api" *;
        export def result = match Choice.pick { pick => used, Choice.skip => 0 };
        export def local = fn() { match 0 { binder => binder } };
    "#).unwrap();
    let resolver = session_workspace_resolver(&directory, &["@src/main", "@src/api"]);
    let root = resolver.selected_root().unwrap();
    let mut sources = SourceDatabase::default();
    let graph = ModuleGraph::discover(&resolver, vec![root.clone()], &BTreeMap::new(),
        builtin_list().into_iter().map(|(name, _)| ModuleCName::builtin(name)),
        None, false, &mut sources).unwrap();
    let root = graph.id(&root.id).unwrap();
    let mut names = StaticNames::new(&graph);
    assert_eq!(names.scopes[root.index()].open.len(), 1);
    let provider = names.scopes[root.index()].open[0];
    let exported = ["used", "unused", "Choice", "pick", "skip", "binder"].map(|name|
        (name, names.export_target(provider, name).expect("provider exports indexed before resolve")));
    assert!(names.export_target(provider, "missing").is_none());
    for name in ["used", "unused", "Choice", "pick", "skip", "binder"] {
        assert!(!names.scopes[root.index()].direct.contains_key(name));
    }
    assert!(names.exports.iter().flatten().all(|state| matches!(state, StaticExportState::Unknown)),
        "scope creation must not classify every export");
    let resolved = names.module_resolution(root).unwrap();
    assert!(resolved.diagnostics.is_empty());
    for (name, target) in exported {
        assert_eq!(names.export_target(provider, name), Some(target),
            "resolving a consumer must not change provider identities");
        if let Some(selected) = resolved.imports.get(name) { assert_eq!(*selected, target); }
    }
    for name in ["used", "Choice", "pick"] { assert!(resolved.imports.contains_key(name), "{name}"); }
    for name in ["unused", "skip", "binder"] { assert!(!resolved.imports.contains_key(name), "{name}"); }
    for name in ["unused", "skip"] {
        let StaticImportTarget::Export { module, index } = names.export_target(provider, name).unwrap()
            else { panic!("expected export row"); };
        assert!(matches!(names.exports[module.index()][index as usize], StaticExportState::Unknown),
            "unreferenced exports must not become classification work");
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn session_export_aliases_share_source_identity_without_merging_new_definitions() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(directory.join("src/model.telora"), r#"
        export def original = 1;
        export { original as alias };
        export def separate = original;
        export def identity = fn(value) { value };
        export { identity as identity_alias };
    "#).unwrap();
    fs::write(directory.join("src/bridge.telora"), r#"
        import "./model" { alias as forwarded };
        import "./model" as model;
        export { forwarded, model };
    "#).unwrap();
    fs::write(directory.join("src/main.telora"), r#"
        import "./model" { original, alias, separate, identity, identity_alias };
        import "./bridge" { forwarded, model };
        import "./bridge" as bridge;
        export def result = original + alias + forwarded + separate + model.original;
        export def distinct_instantiations = (identity(1), identity_alias("text"));
        export def namespace_instantiations = (model.identity("text"), bridge.model.identity(1));
    "#).unwrap();
    let resolver = session_workspace_resolver(&directory, &["@src/main", "@src/model", "@src/bridge"]);
    let root = resolver.selected_root().unwrap();
    let mut sources = SourceDatabase::default();
    let graph = ModuleGraph::discover(&resolver, vec![root.clone()], &BTreeMap::new(),
        builtin_list().into_iter().map(|(name, _)| ModuleCName::builtin(name)),
        None, false, &mut sources).unwrap();
    let root = graph.id(&root.id).unwrap();
    let modules = StaticNames::new(&graph).resolve(root).modules;
    let resolved = modules[root.index()].as_ref().unwrap();
    assert!(resolved.diagnostics.is_empty(), "{:?}", resolved.diagnostics);
    assert_eq!(resolved.imports["original"], resolved.imports["alias"]);
    assert_eq!(resolved.imports["original"], resolved.imports["forwarded"]);
    let origin = |name: &str| {
        let reference = resolved.hir.references().iter().find(|reference| reference.name == name).unwrap();
        resolved.hir.reference_import_origin(reference.id).unwrap()
    };
    assert_eq!(origin("original"), origin("forwarded"));
    assert!(matches!(origin("original"), crate::hir::HirImportOrigin::Definition { .. }));
    assert_eq!(origin("identity"), origin("identity_alias"));
    let members = resolved.hir.member_accesses().iter().filter(|member| member.field == "identity")
        .collect::<Vec<_>>();
    assert_eq!(members.len(), 2);
    for member in members {
        let expression = resolved.hir.expression(member.expression).unwrap();
        assert_eq!(resolved.hir.expression_import_origin_at(expression.location), Some(origin("identity")),
            "direct and nested namespace access must bind the same source declaration");
    }
    assert_ne!(origin("original"), origin("separate"));
    assert_eq!(resolved.imports["model"], StaticImportTarget::Namespace(resolved.imports["original"].module()));
    assert_ne!(resolved.imports["original"], resolved.imports["separate"],
        "equal values/types must not merge independently authored definitions");
    let snapshot = recovery_engine().check_types_with_resolver(resolver).unwrap();
    assert!(snapshot.diagnostics().is_empty(), "{:?}", snapshot.diagnostics());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn namespace_missing_member_is_resolved_before_types_and_respects_shadowing() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(directory.join("src/provider.telora"), "export def known: Int = \"bad\";").unwrap();
    fs::write(directory.join("src/main.telora"), r#"
        import "./provider" as provider;
        export def missing = provider.absent;
        export def local = fn() { let provider = { absent: 1 }; provider.absent };
    "#).unwrap();
    let resolver = session_workspace_resolver(&directory, &["@src/main", "@src/provider"]);
    let root = resolver.selected_root().unwrap();
    let mut sources = SourceDatabase::default();
    let graph = ModuleGraph::discover(&resolver, vec![root.clone()], &BTreeMap::new(),
        builtin_list().into_iter().map(|(name, _)| ModuleCName::builtin(name)),
        None, false, &mut sources).unwrap();
    let modules = StaticNames::new(&graph).resolve(graph.id(&root.id).unwrap()).modules;
    let resolved = modules[graph.id(&root.id).unwrap().index()].as_ref().unwrap();
    assert_eq!(resolved.diagnostics.len(), 1, "{:?}", resolved.diagnostics);
    assert!(resolved.diagnostics[0].message.contains("has no export \"absent\""));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn resolve_errors_are_collected_before_any_module_type_solution() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    fs::write(directory.join("src/provider.telora"), "export def value: Int = \"bad\";").unwrap();
    fs::write(directory.join("src/main.telora"), r#"
        import "./provider" as provider;
        export def left = unknown_left;
        export def right = unknown_right;
    "#).unwrap();
    let resolver = session_workspace_resolver(&directory, &["@src/main", "@src/provider"]);
    let snapshot = recovery_engine().check_types_with_resolver(resolver.clone()).unwrap();
    let messages = snapshot.diagnostics().iter().map(|diagnostic| diagnostic.message.as_str()).collect::<Vec<_>>();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages.contains(&"unknown binding \"unknown_left\""));
    assert!(messages.contains(&"unknown binding \"unknown_right\""));
    assert!(snapshot.modules().iter().all(|module| module.export_schemes.is_empty()),
        "a resolve failure must not publish a solution for any module");
    fs::write(directory.join("src/main.telora"),
        "import \"./provider\" as provider; export def result = provider.value;").unwrap();
    let snapshot = recovery_engine().check_types_with_resolver(resolver).unwrap();
    assert!(snapshot.diagnostics().iter().any(|diagnostic| diagnostic.message.contains("Int")),
        "type diagnostics become available after resolve succeeds: {:?}", snapshot.diagnostics());
    fs::remove_dir_all(directory).unwrap();
}
