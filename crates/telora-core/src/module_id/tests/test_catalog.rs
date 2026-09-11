fn test_catalog_fixture(label: &str) -> (PathBuf, PathBuf) {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("telora-test-catalog-{label}-{unique}"));
    let app = root.join("member");
    std::fs::create_dir_all(app.join("src")).unwrap();
    std::fs::create_dir_all(app.join("tests/helpers/nested")).unwrap();
    for path in [
        "src/lib.telora",
        "tests/a.telora",
        "tests/b.telora",
        "tests/helpers/common.telora",
        "tests/helpers/nested/deep.telora",
    ] {
        std::fs::write(app.join(path), "export def value = 1;").unwrap();
    }
    std::fs::write(app.join("tests/helpers/input.json"), "{}").unwrap();
    write_test_workspace(&root, &[("member", "app", &[])]);
    (root, app)
}

#[test]
fn test_catalog_imports_share_identity_and_preserve_visibility() {
    let (root, app) = test_catalog_fixture("visibility");
    std::fs::create_dir_all(app.join("tests/helpers/tests")).unwrap();
    std::fs::write(
        app.join("tests/helpers/tests/child.telora"),
        "export def value = 1;",
    )
    .unwrap();
    std::fs::create_dir_all(app.join("src/tests")).unwrap();
    std::fs::write(
        app.join("src/tests/source_only.telora"),
        "export def value = 1;",
    )
    .unwrap();
    write_test_workspace(&root, &[("member", "app", &[])]);
    let resolver = ModuleResolver::from_cwd(&app, "@test/a").unwrap();
    let a = resolver.selected_root().unwrap();
    let common = resolver.resolve_import(&a.id, "./helpers/common").unwrap();
    assert!(
        resolver
            .resolve_import(&a.id, "./helpers/tests/child")
            .is_ok()
    );
    assert!(
        resolver
            .resolve_import(&a.id, "@src/tests/source_only")
            .is_ok()
    );
    assert_eq!(common.id.to_string(), "app/tests/helpers/common");
    for spelling in [
        "@test/helpers/common",
        "app/tests/helpers/common",
        "./helpers/nested/../common",
    ] {
        assert_eq!(resolver.resolve_import(&a.id, spelling).unwrap(), common);
    }
    assert_eq!(resolver.resolve_import(&common.id, "../a").unwrap(), a);
    assert_eq!(resolver.resolve_import(&a.id, "./a").unwrap(), a);
    assert_eq!(
        resolver
            .resolve_import(&common.id, "../b")
            .unwrap()
            .id
            .to_string(),
        "app/tests/b"
    );
    assert_eq!(
        resolver
            .resolve_import(&common.id, "./input.json")
            .unwrap()
            .format,
        ModuleFormat::Json
    );
    let source = resolver.resolve_import(&a.id, "@src/lib").unwrap();
    for importer in [
        source.id,
        ModuleCName::Dependency {
            name: "dep".into(),
            path: "lib".into(),
        },
    ] {
        for target in ["@test/a", "app/tests/a", "@test/helpers/common"] {
            assert!(resolver.resolve_import(&importer, target).is_err());
        }
    }
    for target in [
        "../a",
        "@test/../src/lib",
        "dep/tests/a",
        "./b.telora",
        "./helpers/input.txt",
    ] {
        assert!(resolver.resolve_import(&a.id, target).is_err(), "{target}");
    }
    std::fs::write(app.join("src/undeclared.telora"), "export def value = 1;").unwrap();
    assert!(resolver.resolve_import(&a.id, "@src/undeclared").is_err());
    let nested = ModuleResolver::for_root(&app.join("tests/helpers/nested/deep.telora")).unwrap();
    assert_eq!(
        nested.selected_root().unwrap().id.to_string(),
        "app/tests/helpers/nested/deep"
    );
    assert!(
        ModuleResolver::catalog_from_cwd(&app, [])
            .unwrap()
            .iter()
            .all(|module| !matches!(module.id, ModuleCName::Test { .. }))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_catalog_rejects_ambiguous_canonical_names() {
    let (root, app) = test_catalog_fixture("identity-conflict");
    std::fs::create_dir_all(app.join("src/tests")).unwrap();
    std::fs::write(app.join("src/tests/a.telora"), "export def value = 1;").unwrap();
    write_test_workspace(&root, &[("member", "app", &[])]);
    let error = ModuleResolver::from_cwd(&app, "@test/a").unwrap_err();
    assert!(error.to_string().contains("identity conflicts"), "{error}");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_catalog_is_fixed_without_parsing_unreachable_files() {
    let (root, app) = test_catalog_fixture("fixed");
    std::fs::write(app.join("tests/broken.telora"), "not valid source !!!").unwrap();
    let resolver = ModuleResolver::from_cwd(&app, "@test/a").unwrap();
    let a = resolver.selected_root().unwrap();
    assert!(resolver.resolve_import(&a.id, "./broken").is_ok());
    std::fs::write(app.join("tests/late.telora"), "export def value = 1;").unwrap();
    assert!(resolver.resolve_import(&a.id, "./late").is_err());
    assert!(resolver.clone().resolve_import(&a.id, "./late").is_err());
    assert!(
        ModuleResolver::from_cwd(&app, "@test/a")
            .unwrap()
            .resolve_import(&a.id, "./late")
            .is_ok()
    );
    std::fs::remove_file(app.join("tests/b.telora")).unwrap();
    assert!(resolver.resolve_import(&a.id, "./b").is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn test_catalog_rejects_symlinks_and_path_replacement() {
    use std::os::unix::fs::symlink;
    let (root, app) = test_catalog_fixture("links");
    let resolver = ModuleResolver::from_cwd(&app, "@test/a").unwrap();
    let a = resolver.selected_root().unwrap();
    std::fs::remove_file(app.join("tests/b.telora")).unwrap();
    symlink(app.join("src/lib.telora"), app.join("tests/b.telora")).unwrap();
    assert!(resolver.resolve_import(&a.id, "./b").is_err());
    assert!(ModuleResolver::from_cwd(&app, "@test/a").is_err());
    assert!(ModuleResolver::for_root(&app.join("tests/b.telora")).is_err());
    std::fs::remove_file(app.join("tests/b.telora")).unwrap();
    symlink(app.join("tests"), app.join("tests/helpers/loop")).unwrap();
    assert!(ModuleResolver::from_cwd(&app, "@test/a").is_err());
    std::fs::remove_file(app.join("tests/helpers/loop")).unwrap();
    std::fs::rename(app.join("tests"), app.join("actual-tests")).unwrap();
    symlink(app.join("actual-tests"), app.join("tests")).unwrap();
    assert!(ModuleResolver::from_cwd(&app, "@test/a").is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_catalog_cycle_edges_retain_canonical_module_identity() {
    let (root, app) = test_catalog_fixture("strict-cycle");
    std::fs::write(
        app.join("tests/a.telora"),
        "import \"./b\" as b; export def value = 1;",
    )
    .unwrap();
    std::fs::write(
        app.join("tests/b.telora"),
        "import \"app/tests/a\" as a; export def value = 2;",
    )
    .unwrap();
    let resolver = ModuleResolver::from_cwd(&app, "@test/a").unwrap();
    let a = resolver.selected_root().unwrap();
    let b = resolver.resolve_import(&a.id, "./b").unwrap();
    let back = resolver.resolve_import(&b.id, "app/tests/a").unwrap();
    assert_eq!(a.id, back.id);
    assert_eq!(a.path(), back.path());
    std::fs::remove_dir_all(root).unwrap();
}
