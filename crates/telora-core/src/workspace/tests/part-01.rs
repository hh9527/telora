    #[test]
    fn overlay_revisions_are_cow_and_publish_atomically() {
        let (directory, root) = fixture("let disk = 1; disk");
        let workspace = Workspace::new(&root, engine()).unwrap();
        let old_context = workspace.context();
        let revision = workspace
            .open(&root, DocumentVersion(1), "let overlay = 2; overlay")
            .unwrap();
        assert_eq!(revision, Revision(1));
        assert!(matches!(
            old_context.check(),
            Err(QueryError::StaleRevision { .. })
        ));
        let old_document = workspace.document(&root).unwrap();

        let revision = workspace
            .change(
                &root,
                DocumentVersion(1),
                DocumentVersion(2),
                &[TextEdit::Replace {
                    range: TextRange::new(14, 15).unwrap(),
                    replacement: "3".into(),
                }],
            )
            .unwrap();
        assert_eq!(revision, Revision(2));
        assert_eq!(old_document.text().to_string(), "let overlay = 2; overlay");
        assert_eq!(
            workspace.document(&root).unwrap().text().to_string(),
            "let overlay = 3; overlay"
        );

        let context = workspace.context();
        let snapshot = block_on(workspace.rebuild(&context)).unwrap();
        assert_eq!(snapshot.revision(), Revision(2));
        assert!(
            snapshot
                .definitions()
                .iter()
                .any(|definition| definition.name == "overlay")
        );
        assert!(
            !snapshot
                .definitions()
                .iter()
                .any(|definition| definition.name == "disk")
        );
        assert_eq!(workspace.published().unwrap().revision(), Revision(2));

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cancelled_and_stale_builds_do_not_publish() {
        let (directory, root) = fixture("1");
        let workspace = Workspace::new(&root, engine()).unwrap();
        let cancellation = CancellationToken::default();
        let context = workspace.cancellable_context(cancellation.clone());
        cancellation.cancel();
        assert!(matches!(
            block_on(workspace.rebuild(&context)),
            Err(WorkspaceError::Query(QueryError::Cancelled))
        ));
        assert!(workspace.published().is_none());

        let stale = workspace.context();
        workspace.open(&root, DocumentVersion(1), "2").unwrap();
        assert!(matches!(
            block_on(workspace.rebuild(&stale)),
            Err(WorkspaceError::Query(QueryError::StaleRevision { .. }))
        ));
        assert!(workspace.published().is_none());

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn valid_overlay_dependencies_supply_real_import_capabilities() {
        let (directory, root) = fixture(
            "import \"./model.telora\" as model; type FromOverlay = model.Shared; export { FromOverlay as output };",
        );
        let model = directory.join("model.telora");
        std::fs::write(&model, "type Shared = missing; 0").unwrap();
        let workspace = Workspace::new(&root, engine()).unwrap();
        workspace
            .open(
                &model,
                DocumentVersion(1),
                "type Shared = String; export { Shared };",
            )
            .unwrap();
        let context = workspace.context();
        let snapshot = block_on(workspace.rebuild(&context)).unwrap();
        let root_module = snapshot
            .module_by_path(&std::fs::canonicalize(&root).unwrap())
            .unwrap();
        let fact = &snapshot
            .definitions()
            .iter()
            .find(|definition| {
                definition.module == root_module.id && definition.name == "FromOverlay"
            })
            .unwrap()
            .ty;
        assert_eq!(fact.state, crate::FactState::Known);
        assert_eq!(
            snapshot.types().display(fact.value.unwrap()).unwrap(),
            "TypeOf(String)"
        );
        std::fs::remove_dir_all(directory).unwrap();
    }
