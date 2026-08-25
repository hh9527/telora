    #[test]
    fn formats_logical_ids_without_physical_paths() {
        let id = ModuleCName::Source {
            owner: "app".into(),
            path: PathBuf::from("a file.yml"),
        };
        assert_eq!(id.to_string(), "app/a file.yml");
        assert_eq!(
            ModuleFormat::from_path(Path::new("a.yaml")),
            Ok(ModuleFormat::Yaml)
        );
        assert_eq!(
            ModuleFormat::from_path(Path::new("a.yml")),
            Ok(ModuleFormat::Yaml)
        );
        assert!(ModuleFormat::from_path(Path::new("a.JSON")).is_err());
    }

    #[test]
    fn catalog_exposes_the_crate_internals_and_only_external_public_modules() {
        let temporary =
            std::env::temp_dir().join(format!("telora-module-catalog-test-{}", std::process::id()));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(app.join("src/bin")).unwrap();
        std::fs::create_dir_all(app.join("tests")).unwrap();
        std::fs::create_dir_all(dependency.join("src/bin")).unwrap();
        std::fs::create_dir_all(dependency.join("src/entry")).unwrap();
        std::fs::create_dir_all(dependency.join("tests")).unwrap();
        std::fs::write(app.join("src/lib.telora"), "0").unwrap();
        std::fs::write(app.join("src/_rules.telora"), "0").unwrap();
        std::fs::create_dir_all(app.join("src/entry")).unwrap();
        std::fs::write(app.join("src/entry/serve.telora"), "0").unwrap();
        std::fs::write(app.join("src/schema.json"), "{}").unwrap();
        std::fs::write(app.join("src/ignored.txt"), "ignored").unwrap();
        std::fs::write(app.join("src/bin/tool.telora"), "0").unwrap();
        std::fs::write(app.join("tests/query.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/public.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/_internal.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/bin/tool.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/entry/serve.telora"), "0").unwrap();
        std::fs::write(dependency.join("tests/query.telora"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"name":"app","dependencies":{"dep":{"path":"../dependency"}}}"#,
        )
        .unwrap();

        let catalog = ModuleResolver::catalog_from_cwd(
            &app,
            [
                ("std/string".to_owned(), 1),
                ("std/_rt".to_owned(), 2),
                ("std/_host".to_owned(), 3),
                ("std/entry/default".to_owned(), 4),
            ],
        )
        .unwrap();
        let by_name = catalog
            .into_iter()
            .map(|module| (module.id.to_string(), module))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(
            by_name.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "app/_rules",
                "app/lib",
                "app/schema.json",
                "dep/public",
                "std/string",
            ]
        );
        assert_eq!(
            by_name["app/_rules"].visibility,
            ModuleVisibility::Private
        );
        assert_eq!(
            by_name["dep/public"].origin,
            ModuleCatalogOrigin::Dependency
        );
        assert_eq!(
            by_name["std/string"].origin,
            ModuleCatalogOrigin::Builtin
        );
        assert_eq!(by_name["app/schema.json"].format, ModuleFormat::Json);
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn path_dependencies_keep_logical_identity() {
        let temporary =
            std::env::temp_dir().join(format!("telora-module-id-test-{}", std::process::id()));
        std::fs::create_dir_all(temporary.join("app")).unwrap();
        std::fs::create_dir_all(temporary.join("models")).unwrap();
        std::fs::write(temporary.join("app/main.telora"), "0").unwrap();
        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{"name":"app","dependencies":{"models":{"path":"models"}}}"#,
        )
        .unwrap();
        std::fs::write(temporary.join("models/user.telora"), "0").unwrap();
        let resolver = ModuleResolver::for_root(&temporary.join("app/main.telora")).unwrap();
        let root = resolver
            .resolve_root(&temporary.join("app/main.telora"))
            .unwrap();
        let dependency = resolver.resolve_import(&root.id, "models/user").unwrap();
        assert_eq!(dependency.id.to_string(), "models/user");
        assert_eq!(dependency.format, ModuleFormat::Telora);
        assert!(matches!(
            resolver.resolve_import(&root.id, ""),
            Err(ResolveModuleError::EmptyPath)
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn builtin_vendor_precedes_configured_vendors_and_private_modules_stay_crate_local() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-module-authority-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        let shadow = temporary.join("shadow");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::create_dir_all(dependency.join("src")).unwrap();
        std::fs::create_dir_all(shadow.join("src")).unwrap();
        let main = app.join("src/main.telora");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(app.join("src/_local.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/public.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/_internal.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/_value.json"), "0").unwrap();
        std::fs::write(dependency.join("src/bad.name.json"), "0").unwrap();
        std::fs::write(shadow.join("src/array.telora"), "0").unwrap();
        std::fs::write(shadow.join("src/extension.telora"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"name":"app","dependencies":{"dep":{"path":"../dependency"},"std":{"path":"../shadow"}}}"#,
        )
        .unwrap();

        let entry = ModuleCName::builtin("std/entry/default");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_builtins([
                ("std/array".to_owned(), 5),
                ("std/_rt".to_owned(), 26),
            ])
            .with_entry_context(entry.clone());
        let root = resolver.resolve_root(&main).unwrap();
        let builtin = resolver.resolve_import(&root.id, "std/array").unwrap();
        assert_eq!(builtin.id, ModuleCName::builtin("std/array"));
        assert_eq!(builtin.vendor, ModuleVendor::Builtin);
        assert!(matches!(
            resolver.resolve_import(&root.id, "std/extension"),
            Err(ResolveModuleError::ModuleNotFound(module)) if module == "std/extension"
        ));
        let local = resolver
            .resolve_import(&root.id, "@src/_local")
            .unwrap();
        assert_eq!(local.vendor, ModuleVendor::Configured);

        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/_internal"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/_value.json"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        let entry_private = resolver
            .resolve_import(&entry, "dep/_internal")
            .unwrap();
        assert_eq!(
            entry_private.id,
            ModuleCName::Dependency {
                name: "dep".into(),
                path: "_internal".into(),
            }
        );
        assert!(matches!(
            resolver.resolve_import(&root.id, "std/_rt"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        let entry_native = resolver
            .resolve_import(&entry, "std/_rt")
            .unwrap();
        assert_eq!(entry_native.vendor, ModuleVendor::Builtin);
        assert_eq!(entry_native.to_string(), "std/_rt");
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/bad.name.json"),
            Err(ResolveModuleError::InvalidModuleSuffix(_))
        ));
        std::fs::remove_file(dependency.join("src/bad.name.json")).unwrap();
        let catalog = ModuleResolver::catalog_from_cwd(
            &app,
            [
                ("std/array".to_owned(), 5),
                ("std/_rt".to_owned(), 26),
            ],
        )
        .unwrap();
        assert!(
            catalog
                .iter()
                .all(|module| module.id.to_string() != "std/extension")
        );
        let dependency_module = resolver.resolve_import(&root.id, "dep/public").unwrap();
        assert!(matches!(
            resolver.resolve_import(&dependency_module.id, "@unknown"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        let internal = resolver
            .resolve_import(&dependency_module.id, "./_internal")
            .unwrap();
        assert_eq!(internal.vendor, ModuleVendor::Configured);
        assert!(matches!(
            ModuleResolver::for_root(&app.join("src/_local.telora"))
                .and_then(|resolver| resolver.resolve_root(&app.join("src/_local.telora"))),
            Err(ResolveModuleError::PrivateModuleRoot)
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn crate_layout_resolves_binary_source_and_contextual_source_roots() {
        let temporary =
            std::env::temp_dir().join(format!("telora-crate-layout-test-{}", std::process::id()));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(app.join("src/model")).unwrap();
        std::fs::create_dir_all(app.join("src/bin")).unwrap();
        std::fs::create_dir_all(dependency.join("src/model")).unwrap();
        let main = app.join("src/bin/tool.telora");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(app.join("src/model/a.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/model/a.telora"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"name":"app","dependencies":{"parser":{"path":"../dependency"}}}"#,
        )
        .unwrap();

        let entry = ModuleCName::builtin("std/entry/default");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_builtins([("std/_rt".to_owned(), 26)])
            .with_entry_context(entry.clone());
        let root = resolver.resolve_root(&main).unwrap();
        assert_eq!(
            root.id,
            ModuleCName::Binary {
                owner: "app".into(),
                path: PathBuf::from("tool"),
            }
        );
        assert_eq!(root.to_string(), "app/bin/tool");

        let local = resolver
            .resolve_import(&root.id, "@src/model/a")
            .unwrap();
        assert_eq!(local.to_string(), "app/model/a");
        assert!(matches!(
            resolver.resolve_import(&root.id, "./model/a"),
            Err(ResolveModuleError::InvalidImport(message))
                if message.contains("binary and test roots must import crate sources with @src/...")
        ));

        let dependency = resolver
            .resolve_import(&root.id, "parser/model/a")
            .unwrap();
        assert_eq!(dependency.to_string(), "parser/model/a");
        assert_eq!(
            resolver
                .resolve_import(&dependency.id, "@src/model/a")
                .unwrap()
                .id,
            dependency.id
        );
        assert!(matches!(
            resolver.resolve_import(&local.id, "@bin/tool"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        assert_eq!(entry.to_string(), "std/entry/default");
        assert_eq!(
            resolver
                .resolve_import(&entry, "@bin/tool")
                .unwrap()
                .id,
            ModuleCName::Binary {
                owner: "app".into(),
                path: PathBuf::from("tool"),
            }
        );
        assert_eq!(
            resolver
                .resolve_import(&entry, "std/_rt")
                .unwrap()
                .id,
            ModuleCName::builtin("std/_rt")
        );
        assert_eq!(
            resolver
                .resolve_import(&ModuleCName::builtin("std/entry/helper"), "std/_rt")
                .unwrap()
                .id,
            ModuleCName::builtin("std/_rt")
        );
        assert!(matches!(
            resolver.resolve_import(&root.id, "std/_rt"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&root.id, "@entry"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn special_roots_are_host_selected_files_only_and_not_ordinary_imports() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-special-root-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(app.join("src/bin/nested")).unwrap();
        std::fs::create_dir_all(app.join("src/entry/nested")).unwrap();
        std::fs::create_dir_all(app.join("tests/nested")).unwrap();
        std::fs::create_dir_all(dependency.join("src/entry")).unwrap();
        std::fs::write(app.join("src/lib.telora"), "0").unwrap();
        std::fs::write(app.join("src/bin/main.telora"), "0").unwrap();
        std::fs::write(app.join("src/bin/other.telora"), "0").unwrap();
        std::fs::write(app.join("src/bin/nested/tool.telora"), "0").unwrap();
        std::fs::write(app.join("src/entry/tool.telora"), "0").unwrap();
        std::fs::write(app.join("src/entry/nested/tool.telora"), "0").unwrap();
        std::fs::write(app.join("tests/nested/query.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/helper.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/entry/tool.telora"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"name":"app","dependencies":{"dep":{"path":"../dependency"}}}"#,
        )
        .unwrap();

        let main = app.join("src/bin/main.telora");
        let selected_entry = ModuleCName::Entry {
            owner: "app".into(),
            path: "tool".into(),
        };
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_builtins([("std/entry/default".to_owned(), 1)])
            .with_entry_context(selected_entry.clone());
        let library = ModuleCName::Source {
            owner: "app".into(),
            path: "lib".into(),
        };

        assert!(matches!(
            resolver.resolve_import(&library, "./entry/tool"),
            Err(ResolveModuleError::EntryModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&library, "dep/entry/tool"),
            Err(ResolveModuleError::EntryModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&library, "std/entry/default"),
            Err(ResolveModuleError::EntryModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&selected_entry, "app/bin/other"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        assert_eq!(
            resolver
                .resolve_import(&selected_entry, "app/bin/main")
                .unwrap()
                .id
                .to_string(),
            "app/bin/main"
        );

        let dependency_entry = resolver.resolve_entry("dep/entry/tool").unwrap();
        let dependency_resolver = resolver
            .clone()
            .with_entry_context(dependency_entry.id.clone());
        assert_eq!(
            dependency_resolver
                .resolve_import(&dependency_entry.id, "@src/helper")
                .unwrap()
                .id
                .to_string(),
            "dep/helper"
        );

        for selector in ["@bin/nested/tool", "@test/nested/query"] {
            assert!(matches!(
                ModuleResolver::from_cwd(&app, selector),
                Err(ResolveModuleError::InvalidImport(message)) if message.contains("files only")
            ));
        }
        assert!(matches!(
            ModuleResolver::for_root(&app.join("src/entry/nested/tool.telora")),
            Err(ResolveModuleError::InvalidImport(message)) if message.contains("files only")
        ));

        let catalog = ModuleResolver::catalog_from_cwd(
            &app,
            [("std/entry/default".to_owned(), 1)],
        )
        .unwrap();
        assert_eq!(
            catalog
                .into_iter()
                .map(|module| module.id.to_string())
                .collect::<Vec<_>>(),
            ["app/lib", "dep/helper"]
        );

        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn logical_roots_reject_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let temporary = std::env::temp_dir().join(format!(
            "telora-logical-root-symlink-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        std::fs::create_dir_all(app.join("src/bin")).unwrap();
        std::fs::create_dir_all(app.join("tests")).unwrap();
        std::fs::write(app.join("telora-deps.json"), r#"{"name":"app"}"#).unwrap();
        std::fs::write(temporary.join("outside.telora"), "export def output = 1;").unwrap();
        symlink(
            temporary.join("outside.telora"),
            app.join("src/bin/escape.telora"),
        )
        .unwrap();
        symlink(
            temporary.join("outside.telora"),
            app.join("tests/escape.telora"),
        )
        .unwrap();

        assert!(matches!(
            ModuleResolver::from_cwd(&app, "@bin/escape"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        assert!(matches!(
            ModuleResolver::from_cwd(&app, "@test/escape"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn local_aliases_share_one_identity_and_formats_are_exact() {
        let temporary =
            std::env::temp_dir().join(format!("telora-module-alias-test-{}", std::process::id()));
        std::fs::create_dir_all(temporary.join("app/sub")).unwrap();
        let main = temporary.join("app/main.telora");
        let data = temporary.join("app/data.json");
        std::fs::write(&main, "option \"crate.name\" \"app\"; 0").unwrap();
        std::fs::write(&data, "{}").unwrap();
        let resolver = ModuleResolver::for_root(&main).unwrap();
        let root = resolver.resolve_root(&main).unwrap();
        let dotted = resolver
            .resolve_import(&root.id, "./sub/../data.json")
            .unwrap();
        let absolute = resolver.resolve_import(&root.id, "@src/data.json").unwrap();
        assert_eq!(dotted, absolute);
        assert_eq!(dotted.format, ModuleFormat::Json);
        assert!(ModuleFormat::from_path(Path::new("data.JSON")).is_err());
        assert!(ModuleFormat::from_path(Path::new("data")).is_err());
        assert!(ModuleFormat::from_path(Path::new("data.txt")).is_err());
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn standalone_uses_a_stable_default_crate_name() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-standalone-default-name-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temporary).unwrap();
        let root = temporary.join("main.telora");
        std::fs::write(&root, "export def value = 1;").unwrap();

        let resolver = ModuleResolver::standalone(&root).unwrap();
        assert_eq!(
            resolver.selected_root().unwrap().id.to_string(),
            "standalone/bin/main"
        );

        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn standalone_overlay_options_build_the_complete_resolve_graph() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-standalone-overlay-test-{}",
            std::process::id()
        ));
        let dependency = temporary.join("dep");
        std::fs::create_dir_all(&dependency).unwrap();
        std::fs::write(dependency.join("value.telora"), "export def value = 42;").unwrap();
        let root = temporary.join("main.telora");
        let source = crate::DocumentText::new(
            r#"option "crate.name" "standalone";
option "crate.dependency" {name: "dep", source: 'Path({path: "dep"})};
import "dep/value" {value};
export {value};"#,
        );

        let resolver = ModuleResolver::for_root_with_source(&root, &source).unwrap();
        let selected = resolver.resolve_root(&root).unwrap();
        assert_eq!(selected.id.to_string(), "standalone/bin/main");
        let imported = resolver.resolve_import(&selected.id, "dep/value").unwrap();
        assert_eq!(imported.id.to_string(), "dep/value");
        let expected = std::fs::canonicalize(dependency.join("value.telora")).unwrap();
        assert_eq!(imported.path(), Some(expected.as_path()));

        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn standalone_relative_imports_follow_the_importing_module() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-standalone-relative-import-test-{}",
            std::process::id()
        ));
        let dependency = temporary.join("dep");
        std::fs::create_dir_all(temporary.join("nested")).unwrap();
        std::fs::create_dir_all(dependency.join("sub")).unwrap();
        std::fs::write(temporary.join("main.telora"), "0").unwrap();
        std::fs::write(temporary.join("nested/first.telora"), "0").unwrap();
        std::fs::write(temporary.join("nested/second.telora"), "0").unwrap();
        std::fs::write(dependency.join("sub/first.telora"), "0").unwrap();
        std::fs::write(dependency.join("sub/second.telora"), "0").unwrap();

        let resolver = ModuleResolver::standalone(&temporary.join("main.telora"))
            .unwrap()
            .with_builtins([]);
        let nested = resolver
            .resolve_import(
                &ModuleCName::Source {
                    owner: "standalone".into(),
                    path: PathBuf::from("nested/first"),
                },
                "./second",
            )
            .unwrap();
        assert_eq!(nested.id.to_string(), "standalone/nested/second");

        let source = crate::DocumentText::new(
            r#"option "crate.dependency" {name: "dep", source: 'Path({path: "dep"})}; 0"#,
        );
        let resolver = ModuleResolver::for_root_with_source(&temporary.join("main.telora"), &source)
            .unwrap();
        let dependency_module = resolver
            .resolve_import(
                &ModuleCName::Dependency {
                    name: "dep".into(),
                    path: PathBuf::from("sub/first"),
                },
                "./second",
            )
            .unwrap();
        assert_eq!(dependency_module.id.to_string(), "dep/sub/second");

        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn crate_source_registration_is_first_win_and_immutable() {
        assert!(validate_crate_name("std").is_ok());
        let temporary = std::env::temp_dir().join(format!(
            "telora-crate-owner-collision-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        let second_dependency = temporary.join("second-dependency");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::create_dir_all(&dependency).unwrap();
        std::fs::create_dir_all(&second_dependency).unwrap();
        std::fs::write(app.join("src/main.telora"), "0").unwrap();
        std::fs::write(app.join("src/value.telora"), "0").unwrap();
        std::fs::write(dependency.join("value.telora"), "0").unwrap();
        std::fs::write(second_dependency.join("value.telora"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"name":"app","dependencies":{"app":{"path":"../dependency"}}}"#,
        )
        .unwrap();
        let resolver = ModuleResolver::for_root(&app.join("src/main.telora")).unwrap();
        let root = resolver.selected_root().unwrap();
        let selected = resolver.resolve_import(&root.id, "app/value").unwrap();
        assert_eq!(selected.id.to_string(), "app/value");
        assert_eq!(
            selected.path().unwrap(),
            std::fs::canonicalize(app.join("src/value.telora"))
                .unwrap()
                .as_path()
        );

        std::fs::write(temporary.join("value.telora"), "0").unwrap();
        let source = crate::DocumentText::new(
            r#"option "crate.dependency" {name: "dep", source: 'Path({path: "dependency"})};
option "crate.dependency" {name: "dep", source: 'Path({path: "second-dependency"})};
option "crate.dependency" {name: "standalone", source: 'Path({path: "dependency"})}; 0"#,
        );
        let resolver =
            ModuleResolver::for_root_with_source(&temporary.join("main.telora"), &source).unwrap();
        let root = resolver.selected_root().unwrap();
        let dependency_value = resolver.resolve_import(&root.id, "dep/value").unwrap();
        assert_eq!(
            dependency_value.path().unwrap(),
            std::fs::canonicalize(dependency.join("value.telora"))
                .unwrap()
                .as_path()
        );
        let standalone_value = resolver
            .resolve_import(&root.id, "standalone/value")
            .unwrap();
        assert_eq!(
            standalone_value.path().unwrap(),
            std::fs::canonicalize(temporary.join("value.telora"))
                .unwrap()
                .as_path()
        );

        std::fs::write(app.join("telora-deps.json"), r#"{"name":"std"}"#).unwrap();
        let resolver = ModuleResolver::for_root(&app.join("src/main.telora"))
            .unwrap()
            .with_builtins([("std/array".to_owned(), 5)]);
        assert!(matches!(
            resolver.selected_root(),
            Err(ResolveModuleError::InvalidImport(message))
                if message.contains("earlier resolver vendor")
        ));

        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn json_manifest_validates_shape_and_exact_data_suffixes() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-module-manifest-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&dependency).unwrap();
        let main = app.join("main.telora");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(dependency.join("schema.json"), "{}").unwrap();
        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{
                "name": "app",
                "dependencies": {"dep": {"path": "dependency"}}
            }"#,
        )
        .unwrap();
        let resolver = ModuleResolver::for_root(&main).unwrap();
        let root = resolver.resolve_root(&main).unwrap();
        let schema = resolver.resolve_import(&root.id, "dep/schema.json").unwrap();
        assert_eq!(schema.format, ModuleFormat::Json);

        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{"name":"app","dependencies": []}"#,
        )
        .unwrap();
        assert!(matches!(
            ModuleResolver::for_root(&main),
            Err(ResolveModuleError::Manifest(message))
                if message.contains("dependencies") && message.contains("object")
        ));

        std::fs::write(temporary.join("telora-deps.json"), "{").unwrap();
        assert!(matches!(
            ModuleResolver::for_root(&main),
            Err(ResolveModuleError::Manifest(message))
                if message.contains("invalid") && message.contains("telora-deps.json")
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn dependency_resolution_rejects_lexical_and_symlink_escapes() {
        use std::os::unix::fs::symlink;

        let temporary =
            std::env::temp_dir().join(format!("telora-module-escape-test-{}", std::process::id()));
        let app = temporary.join("app");
        let dependency = temporary.join("dependency");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&dependency).unwrap();
        std::fs::write(app.join("main.telora"), "0").unwrap();
        std::fs::write(temporary.join("outside.telora"), "0").unwrap();
        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{"name":"app","dependencies":{"dep":{"path":"dependency"}}}"#,
        )
        .unwrap();
        symlink(
            temporary.join("outside.telora"),
            dependency.join("escape.telora"),
        )
        .unwrap();
        let resolver = ModuleResolver::for_root(&app.join("main.telora")).unwrap();
        let root = resolver.resolve_root(&app.join("main.telora")).unwrap();
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/../outside"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/escape"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }
