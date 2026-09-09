#[test]
fn session_discovery_source_is_reused_after_files_change() {
    let directory = fixture_dir();
    let path = directory.join("main.telora");
    fs::write(
        directory.join("shared.telora"),
        "export def value: Int = 42;",
    )
    .unwrap();
    fs::write(&path, "import \"./shared\" {value}; export {value};").unwrap();
    let resolver = ModuleResolver::for_root(&path)
        .unwrap()
        .with_builtins(builtin_list());
    let root = resolver.selected_root().unwrap();
    let mut sources = SourceDatabase::default();
    let graph = ModuleGraph::discover(
        &resolver,
        vec![root.clone()],
        &BTreeMap::new(),
        builtin_list()
            .into_iter()
            .map(|(name, _)| ModuleCName::builtin(name)),
        None,
        false,
        &mut sources,
    )
    .unwrap();
    let prepared = Arc::clone(&graph.prepared[&root.id]);
    let source_id = prepared.source_id;
    let mut main = MainWorld::with_modules(graph);
    let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
    let builtin_modules = install_native_modules(&mut main, &mut sources, &debug_sink).unwrap();
    let mut loader = ModuleLoader {
        resolver,
        cache: HashMap::new(),
        builtin_modules,
        main,
        visiting: Vec::new(),
        dependencies: BTreeSet::new(),
        module_quota: Quota::with_fuel(1_000_000),
        data_limits: DataLimits::default(),
        debug_sink,
        sources,
        semantic_inputs: BTreeMap::new(),
        source_policy: ModuleSourcePolicy::ExplicitExports,
    };
    // Neither new text nor a now-invalid dependency can change the session's
    // source identities after discovery. A new session will see those edits.
    fs::write(&path, "this is invalid source").unwrap();
    fs::write(directory.join("shared.telora"), "this is invalid too").unwrap();
    let (_, compiled) = loader.compile_root(root.clone(), BTreeMap::new()).unwrap();
    assert!(
        compiled
            .analysis
            .module_interface
            .exports
            .contains_key("value")
    );
    assert!(Arc::ptr_eq(
        &prepared,
        &loader.main.modules.prepared[&root.id]
    ));
    assert_eq!(
        loader.semantic_inputs[&root.id.to_string()].source,
        Some(source_id)
    );
    assert_eq!(
        loader
            .sources
            .files()
            .filter(|source| source.name.as_ref() == root.id.to_string())
            .count(),
        1
    );
    assert!(
        recovery_engine()
            .load_module(&path, BTreeMap::new())
            .is_err()
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn session_discovery_retains_invalid_overlay_for_recovery() {
    let directory = fixture_dir();
    let path = directory.join("main.telora");
    fs::write(&path, "export def value = 42;").unwrap();
    let resolver = ModuleResolver::for_root(&path)
        .unwrap()
        .with_builtins(builtin_list());
    let root = resolver.selected_root().unwrap();
    let overlay = crate::document::DocumentText::new("export def value = ;");
    let overlays = BTreeMap::from([(path.canonicalize().unwrap(), overlay)]);
    let mut sources = SourceDatabase::default();
    let graph = ModuleGraph::discover(
        &resolver,
        vec![root.clone()],
        &BTreeMap::new(),
        builtin_list()
            .into_iter()
            .map(|(name, _)| ModuleCName::builtin(name)),
        Some(&overlays),
        true,
        &mut sources,
    )
    .unwrap();
    let parsed = &graph.prepared[&root.id];
    assert!(graph.id(&root.id).is_some());
    assert!(parsed.program.is_none());
    assert!(!parsed.diagnostics.is_empty());
    assert_eq!(
        sources.get(parsed.source_id).text().to_string(),
        "export def value = ;"
    );
    assert_eq!(parsed.recovered.location.source, parsed.source_id);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn session_import_aliases_share_target_before_value_initialization() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    let path = directory.join("src/main.telora");
    let dependency = directory.join("src/shared.telora");
    fs::write(
        &dependency,
        "export def value: Int = fail!(\"must not initialize\");",
    )
    .unwrap();
    fs::write(
        &path,
        "import \"./shared\" as first; import \"./shared\" as second; export {first, second};",
    )
    .unwrap();
    let resolver = session_workspace_resolver(&directory, &["@src/main", "@src/shared"]);
    let root = resolver.selected_root().unwrap();
    let mut sources = SourceDatabase::default();
    let graph = ModuleGraph::discover(
        &resolver,
        vec![root.clone()],
        &BTreeMap::new(),
        builtin_list()
            .into_iter()
            .map(|(name, _)| ModuleCName::builtin(name)),
        None,
        false,
        &mut sources,
    )
    .unwrap();
    let imports = &graph.prepared[&root.id]
        .program
        .as_ref()
        .unwrap()
        .value
        .body
        .value
        .bindings;
    let first = imports[0].value.value.location;
    let second = imports[1].value.value.location;
    let id = graph.import_targets.target(first).unwrap().unwrap();
    assert_eq!(graph.import_targets.target(second).unwrap().unwrap(), id);
    let target = graph.resolved[id.index()].as_ref().unwrap();
    assert!(graph.prepared.contains_key(&target.id));
    // The target and its source stay registered without reading or executing it.
    fs::remove_file(&dependency).unwrap();
    assert_eq!(
        &graph
            .resolve_import(&resolver, &root.id, first, "./shared")
            .unwrap(),
        target
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn session_missing_import_remains_a_fact_when_catalog_changes() {
    let directory = fixture_dir();
    fs::create_dir_all(directory.join("src")).unwrap();
    let path = directory.join("src/main.telora");
    fs::write(
        &path,
        "import \"std/session-added\" as missing; export def healthy: Int = 42;",
    )
    .unwrap();
    let resolver = session_workspace_resolver(&directory, &["@src/main"]);
    let root = resolver.selected_root().unwrap();
    let mut sources = SourceDatabase::default();
    let graph = ModuleGraph::discover(
        &resolver,
        vec![root.clone()],
        &BTreeMap::new(),
        builtin_list()
            .into_iter()
            .map(|(name, _)| ModuleCName::builtin(name)),
        None,
        true,
        &mut sources,
    )
    .unwrap();
    let location = graph.prepared[&root.id]
        .program
        .as_ref()
        .unwrap()
        .value
        .body
        .value
        .bindings[0]
        .value
        .value
        .location;
    let original_error = graph.import_targets.target(location).unwrap().unwrap_err();
    assert!(!original_error.is_empty());
    let resolver = resolver.with_builtins([("std/session-added".into(), 999)]);
    assert!(
        resolver
            .resolve_import(&root.id, "std/session-added")
            .is_ok()
    );
    assert_eq!(
        graph
            .resolve_import(&resolver, &root.id, location, "std/session-added")
            .unwrap_err()
            .to_string(),
        original_error
    );
    let fresh = ModuleGraph::discover(
        &resolver,
        vec![root.clone()],
        &BTreeMap::new(),
        builtin_list()
            .into_iter()
            .map(|(name, _)| ModuleCName::builtin(name)),
        None,
        true,
        &mut SourceDatabase::default(),
    )
    .unwrap();
    assert!(
        fresh
            .import_targets
            .nodes
            .iter()
            .all(|target| matches!(target, ImportTarget::Resolved(_)))
    );
    fs::remove_dir_all(directory).unwrap();
}

fn session_workspace_resolver(directory: &Path, modules: &[&str]) -> ModuleResolver {
    fs::write(
        directory.join("telora-config.json"),
        r#"{"version":1,"members":["."]}"#,
    )
    .unwrap();
    fs::write(
        directory.join("telora-crate.json"),
        serde_json::to_string(&serde_json::json!({
            "name": "session-graph", "modules": modules, "dependencies": []
        }))
        .unwrap(),
    )
    .unwrap();
    let workspace = crate::package::WorkspaceSpec::discover(directory).unwrap();
    let lock = workspace.generate_lock(&BTreeMap::new()).unwrap();
    workspace.write_lock(&lock).unwrap();
    ModuleResolver::from_cwd(directory, "@src/main")
        .unwrap()
        .with_builtins(builtin_list())
}

#[test]
fn import_graph_refines_stable_nodes_and_keeps_conflicts() {
    let mut sources = SourceDatabase::default();
    let source = sources.add("graph", "abc");
    let first = crate::Location::from_usize(source, 0..1).unwrap();
    let second = crate::Location::from_usize(source, 1..2).unwrap();
    let mut graph = ImportGraph::default();
    let a = graph.register(first);
    let b = graph.register(second);
    assert_eq!(graph.nodes[a.0 as usize], ImportTarget::Pending);
    assert_eq!(graph.register(first), a);
    graph.solve(a, Err("unknown module".into()));
    let target = ModuleId::from_index(0);
    graph.solve(b, Ok(target));
    assert_eq!(graph.register(first), a);
    assert_eq!(graph.target(first), Some(Err("unknown module")));
    assert_eq!(graph.target(second), Some(Ok(target)));
    assert_eq!(graph.nodes.len(), 2);
}
