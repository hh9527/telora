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
