    #[test]
    fn type_members_follow_refs_and_reject_cycles_any_and_unions() {
        let mut types = WorkspaceTypeGraph::default();
        let int = WorkspaceTypeId(0);
        let structure = WorkspaceTypeId(1);
        let reference = WorkspaceTypeId(2);
        let cycle = WorkspaceTypeId(3);
        let any = WorkspaceTypeId(4);
        let union = WorkspaceTypeId(5);
        types.nodes = vec![
            WorkspaceTypeNode::Int,
            WorkspaceTypeNode::Struct(BTreeMap::from([("field".to_owned(), int)])),
            WorkspaceTypeNode::Ref(structure),
            WorkspaceTypeNode::Ref(cycle),
            WorkspaceTypeNode::Any,
            WorkspaceTypeNode::Union(vec![structure]),
        ];
        assert_eq!(types.members_of(reference)[0].name, "field");
        assert!(types.members_of(cycle).is_empty());
        assert!(types.members_of(any).is_empty());
        assert!(types.members_of(union).is_empty());
    }

    #[test]
    fn completion_returns_struct_fields_and_filters_prefixes() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            "let value = {alpha: 1, beta: \"x\"}; let selected = value.alpha; export { selected as output };",
        )
        .unwrap();
        let snapshot = engine().recover_workspace(&main).unwrap();
        let completion = completion_at(&snapshot, "value.alpha").expect("member context");
        assert_eq!(completion.replacement.end - completion.replacement.start, 5);
        assert_eq!(
            completion.candidates,
            vec![CompletionCandidate {
                label: "alpha".to_owned(),
                kind: CompletionKind::StructField,
                ty: completion.candidates[0].ty,
            }]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completion_returns_only_resolved_module_exports() {
        let directory = fixture_dir();
        let model = directory.join("model.telora");
        let main = directory.join("main.telora");
        fs::write(&model, "export def alpha = 1; export def beta = \"x\";").unwrap();
        fs::write(
            &main,
            "import \"./model\" as model; let selected = model.alpha; export { selected as output };",
        )
        .unwrap();
        let snapshot = engine().recover_workspace(&main).unwrap();
        let completion = completion_at(&snapshot, "model.alpha").expect("module context");
        assert_eq!(completion.candidates.len(), 1);
        assert_eq!(completion.candidates[0].label, "alpha");
        assert_eq!(completion.candidates[0].kind, CompletionKind::ModuleExport);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn completion_does_not_recognize_strings_comments_or_bare_names() {
        for source_text in ["\"value.alpha\"", "1 # value.alpha", "let value = 1; value"] {
            let snapshot = WorkspaceSnapshot::recover_source("context.telora", source_text);
            let source = snapshot.modules()[0].source.unwrap();
            let context =
                crate::query::QueryContext::current(crate::query::RevisionClock::default());
            let result = block_on(snapshot.query_completion_at(
                &context,
                Location::new(source, TextRange::at(source_text.len() as u32)),
            ))
            .unwrap();
            assert!(result.is_none(), "unexpected context in {source_text:?}");
        }
    }

    #[test]
    fn indexes_workspace_modules_scopes_types_and_recursive_graphs() {
        let directory = fixture_dir();
        let model = directory.join("model.telora");
        let data = directory.join("data.json");
        let main = directory.join("main.telora");
        fs::write(
            &model,
            "type Node = struct {children: Array(Node)}; export { Node };",
        )
        .unwrap();
        fs::write(&data, "{\"value\":1}").unwrap();
        fs::write(
            &main,
            "import \"./model\" as model;\n\
             import \"./data.json\" as data;\n\
             def f = fn(x) { let y = x; y };\n\
             def count = 1 + 2;\n\
             export { model, data, f, count };",
        )
        .unwrap();

        let loaded = engine().load_module(&main, BTreeMap::new()).unwrap();
        let snapshot = &loaded.workspace;
        assert_eq!(
            snapshot
                .modules()
                .iter()
                .filter(|module| module.kind != WorkspaceModuleKind::Core)
                .count(),
            3
        );
        let main_module = snapshot
            .module_by_path(&fs::canonicalize(&main).unwrap())
            .unwrap();
        let model_import = main_module
            .imports
            .iter()
            .find(|import| import.name == "model")
            .unwrap();
        assert_eq!(
            snapshot
                .module(model_import.target)
                .unwrap()
                .path
                .as_deref(),
            Some(fs::canonicalize(&model).unwrap().as_path())
        );

        let x = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.name == "x")
            .unwrap();
        let x_use = snapshot
            .references()
            .iter()
            .find(|reference| reference.name == "x")
            .unwrap();
        assert_eq!(x_use.definition, Some(x.id));
        let y = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.name == "y")
            .unwrap();
        assert_eq!(snapshot.references_of(y.id).len(), 1);

        let node = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.name == "Node")
            .unwrap();
        let node_type = node.ty.value.unwrap();
        let shown = snapshot.types().display(node_type).unwrap();
        assert_eq!(shown, "TypeOf(Node)");
        assert_eq!(
            snapshot.type_at(Location::new(
                node.location.source,
                TextRange::at(node.location.start),
            )),
            Some(node_type)
        );
        let node_reference = snapshot
            .references()
            .iter()
            .find(|reference| reference.definition == Some(node.id))
            .unwrap();
        assert_eq!(
            snapshot.type_at(Location::new(
                node_reference.location.source,
                TextRange::at(node_reference.location.start),
            )),
            Some(node_type),
            "resolved type references must prefer the promoted definition root"
        );
        assert!(
            snapshot
                .exports_of(main_module.id)
                .iter()
                .any(|item| item.name == "f")
        );
        assert!(
            snapshot
                .expressions()
                .iter()
                .filter(|expression| expression.module == main_module.id)
                .all(|expression| expression.ty.value.is_some())
        );
        let main_source = snapshot.sources().get(main_module.source.unwrap());
        let literal = u32::try_from(main_source.text().to_string().find("1 + 2").unwrap()).unwrap();
        let expression = snapshot
            .expression_at(Location::new(main_source.id(), TextRange::at(literal)))
            .unwrap();
        assert_eq!(
            snapshot.types().node(expression.ty.value.unwrap()),
            Some(&WorkspaceTypeNode::Int)
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recovers_hir_and_unavailable_facts_around_damaged_source() {
        let snapshot = WorkspaceSnapshot::recover_source(
            "damaged.telora",
            "let before = 1; let broken = ; let after = missing; after",
        );
        assert!(!snapshot.diagnostics().is_empty());
        let names = snapshot
            .definitions()
            .iter()
            .map(|definition| definition.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"before"), "{names:?}");
        assert!(names.contains(&"after"), "{names:?}");
        assert!(!names.contains(&"broken"), "{names:?}");
        let missing = snapshot
            .references()
            .iter()
            .find(|reference| reference.name == "missing")
            .expect("complete sibling expression is retained");
        let expression = snapshot
            .expressions()
            .iter()
            .find(|expression| expression.reference == Some(missing.id))
            .unwrap();
        assert_eq!(
            expression.ty.state,
            FactState::Unknown(UnknownReason::UnresolvedName)
        );
    }

    #[test]
    fn known_any_is_distinct_from_unavailable_fact_states() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("any.telora", "let id: Fn(Any) -> Any = fn(x) { x }; id");
        let parsed = crate::parser::parse_registered(&sources, source);
        let program = parsed.program.unwrap();
        let analysis =
            crate::types::analyze_program_registered("any.telora", &sources, &program, 1_000_000)
                .unwrap();
        let snapshot = WorkspaceSnapshot::build(
            sources,
            vec![SemanticModuleInput {
                key: "any.telora".into(),
                path: None,
                kind: WorkspaceModuleKind::Telora,
                source: Some(source),
                program: Some(program),
                analysis: Some(analysis),
                partial: None,
                interface: None,
                state: WorkspaceModuleState::Available,
                imports: Vec::new(),
                diagnostics: Vec::new(),
            }],
        );
        let known = snapshot
            .expressions()
            .iter()
            .find(|expression| {
                expression.ty.state == FactState::Known
                    && expression.ty.value.is_some_and(|ty| {
                        snapshot.types().node(ty) == Some(&WorkspaceTypeNode::Any)
                    })
            })
            .expect("parameter reference has a known Any type");
        let unknown = SemanticFact::<WorkspaceTypeId>::unknown(UnknownReason::MissingSyntax);
        let conflicted =
            SemanticFact::<WorkspaceTypeId>::conflicted(None, Conflict::IncompatibleContract);
        let incomputable =
            SemanticFact::<WorkspaceTypeId>::incomputable(None, IncomputableReason::QuotaExceeded);
        assert_eq!(known.ty.state, FactState::Known);
        assert!(known.ty.value.is_some());
        assert!(matches!(unknown.state, FactState::Unknown(_)));
        assert!(matches!(conflicted.state, FactState::Conflicted(_)));
        assert!(matches!(incomputable.state, FactState::Incomputable(_)));
    }

    #[test]
    fn recovered_duplicate_slots_are_conflicted_with_one_diagnostic() {
        let snapshot = WorkspaceSnapshot::recover_source(
            "conflict.telora",
            "decl item: Int; decl item: String; 0",
        );
        let slots = snapshot
            .definitions()
            .iter()
            .filter(|definition| definition.name == "item")
            .collect::<Vec<_>>();
        assert_eq!(slots.len(), 2);
        assert!(slots.iter().all(|definition| {
            definition.ty.state == FactState::Conflicted(Conflict::DuplicateDefinition)
                && definition.ty.diagnostics.len() == 1
        }));
        assert_eq!(
            snapshot
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("duplicate definition slot"))
                .count(),
            1
        );
    }

    #[test]
    fn recovery_snapshot_projects_partial_type_evaluation_facts() {
        let snapshot = WorkspaceSnapshot::recover_source(
            "partial.telora",
            "type A = broken(Int);\
             type B = String;\
             type C = Array(B);\
             type D = Array(A);\
             0",
        );
        let fact = |name: &str| {
            &snapshot
                .definitions()
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .ty
        };
        assert!(matches!(
            fact("A").state,
            FactState::Incomputable(IncomputableReason::UnsupportedOperation)
        ));
        assert_eq!(fact("B").state, FactState::Known);
        assert_eq!(fact("C").state, FactState::Known);
        assert_eq!(
            snapshot.types().display(fact("C").value.unwrap()).unwrap(),
            "Array<String>"
        );
        let a = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.name == "A")
            .unwrap();
        assert_eq!(
            fact("D").state,
            FactState::Unknown(UnknownReason::BlockedBy(FactIdentity::Definition(a.id)))
        );
    }
