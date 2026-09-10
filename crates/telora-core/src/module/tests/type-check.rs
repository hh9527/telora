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
