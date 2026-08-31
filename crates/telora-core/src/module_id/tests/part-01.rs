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
        write_test_workspace(
            &temporary,
            &[("app", "app", &["dep"]), ("dependency", "dep", &[])],
        );

        let catalog = ModuleResolver::catalog_from_cwd(
            &app,
            [
                ("std/string".to_owned(), 1),
                ("std/_rt".to_owned(), 2),
                ("std/_host".to_owned(), 3),
                ("std/_entry-default".to_owned(), 4),
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
                "app/bin/tool",
                "app/entry/serve",
                "app/lib",
                "app/schema.json",
                "dep/bin/tool",
                "dep/entry/serve",
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
        std::fs::create_dir_all(temporary.join("app/src")).unwrap();
        std::fs::create_dir_all(temporary.join("models/src")).unwrap();
        std::fs::write(temporary.join("app/src/main.telora"), "0").unwrap();
        std::fs::write(temporary.join("models/src/user.telora"), "0").unwrap();
        write_test_workspace(
            &temporary,
            &[("app", "app", &["models"]), ("models", "models", &[])],
        );
        let resolver = ModuleResolver::for_root(&temporary.join("app/src/main.telora")).unwrap();
        let root = resolver
            .resolve_root(&temporary.join("app/src/main.telora"))
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
        write_test_workspace(
            &temporary,
            &[
                ("app", "app", &["dep", "std"]),
                ("dependency", "dep", &[]),
                ("shadow", "std", &[]),
            ],
        );

        let entry = ModuleCName::builtin("std/_entry-default");
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
        write_test_workspace(
            &temporary,
            &[("app", "app", &["parser"]), ("dependency", "parser", &[])],
        );

        let entry = ModuleCName::builtin("std/_entry-default");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_builtins([("std/_rt".to_owned(), 26)])
            .with_entry_context(entry.clone());
        let root = resolver.resolve_root(&main).unwrap();
        assert_eq!(
            root.id,
            ModuleCName::Source {
                owner: "app".into(),
                path: PathBuf::from("bin/tool"),
            }
        );
        assert_eq!(root.to_string(), "app/bin/tool");

        let local = resolver
            .resolve_import(&root.id, "@src/model/a")
            .unwrap();
        assert_eq!(local.to_string(), "app/model/a");
        assert!(resolver.resolve_import(&root.id, "./model/a").is_err());

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
        assert_eq!(entry.to_string(), "std/_entry-default");
        assert_eq!(
            resolver
                .resolve_import(&entry, "app/bin/tool")
                .unwrap()
                .id,
            root.id
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
        write_test_workspace(
            &temporary,
            &[("app", "app", &["dep"]), ("dependency", "dep", &[])],
        );

        let main = app.join("src/bin/main.telora");
        let selected_entry = ModuleCName::builtin("std/_entry-default");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_builtins([("std/_entry-default".to_owned(), 1)])
            .with_entry_context(selected_entry.clone());
        let library = ModuleCName::Source {
            owner: "app".into(),
            path: "lib".into(),
        };

        assert_eq!(
            resolver.resolve_import(&library, "./entry/tool").unwrap().id.to_string(),
            "app/entry/tool"
        );
        assert_eq!(
            resolver.resolve_import(&library, "dep/entry/tool").unwrap().id.to_string(),
            "dep/entry/tool"
        );
        assert!(matches!(
            resolver.resolve_import(&library, "std/_entry-default"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        assert_eq!(
            resolver.resolve_import(&selected_entry, "app/bin/other").unwrap().id.to_string(),
            "app/bin/other"
        );
        assert_eq!(
            resolver
                .resolve_import(&selected_entry, "app/bin/main")
                .unwrap()
                .id
                .to_string(),
            "app/bin/main"
        );

        assert!(matches!(
            ModuleResolver::from_cwd(&app, "@bin/nested/tool"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        assert!(matches!(
            ModuleResolver::from_cwd(&app, "@test/nested/query"),
            Err(ResolveModuleError::InvalidImport(message)) if message.contains("files only")
        ));
        assert!(ModuleResolver::for_root(&app.join("src/entry/nested/tool.telora")).is_ok());

        let catalog = ModuleResolver::catalog_from_cwd(
            &app,
            [("std/_entry-default".to_owned(), 1)],
        )
        .unwrap();
        assert_eq!(
            catalog
                .into_iter()
                .map(|module| module.id.to_string())
                .collect::<Vec<_>>(),
            [
                "app/bin/main",
                "app/bin/nested/tool",
                "app/bin/other",
                "app/entry/nested/tool",
                "app/entry/tool",
                "app/lib",
                "dep/entry/tool",
                "dep/helper",
            ]
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
        write_test_workspace(&temporary, &[("app", "app", &[])]);
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
            ModuleResolver::from_cwd(&app, "@src/bin/escape"),
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
        std::fs::write(&main, "0").unwrap();
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
            "standalone/main"
        );

        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn standalone_relative_imports_follow_the_importing_module() {
        let temporary = std::env::temp_dir().join(format!(
            "telora-standalone-relative-import-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(temporary.join("nested")).unwrap();
        std::fs::write(temporary.join("main.telora"), "0").unwrap();
        std::fs::write(temporary.join("nested/first.telora"), "0").unwrap();
        std::fs::write(temporary.join("nested/second.telora"), "0").unwrap();

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

        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn crate_sources_are_unique_and_builtin_registration_is_first_win() {
        assert!(validate_crate_name("std").is_ok());
        let temporary = std::env::temp_dir().join(format!(
            "telora-crate-owner-collision-test-{}",
            std::process::id()
        ));
        let app = temporary.join("app");
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::write(app.join("src/main.telora"), "0").unwrap();
        std::fs::write(app.join("src/value.telora"), "0").unwrap();
        write_test_workspace(&temporary, &[("app", "app", &[])]);
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

        write_test_workspace(&temporary, &[("app", "std", &[])]);
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
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::create_dir_all(dependency.join("src")).unwrap();
        let main = app.join("src/main.telora");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(dependency.join("src/schema.json"), "{}").unwrap();
        write_test_workspace(
            &temporary,
            &[("app", "app", &["dep"]), ("dependency", "dep", &[])],
        );
        let resolver = ModuleResolver::for_root(&main).unwrap();
        let root = resolver.resolve_root(&main).unwrap();
        let schema = resolver.resolve_import(&root.id, "dep/schema.json").unwrap();
        assert_eq!(schema.format, ModuleFormat::Json);

        std::fs::write(
            app.join(crate::package::CRATE_FILE),
            r#"{"name":"app","modules":["@src/main"],"dependencies": {}}"#,
        )
        .unwrap();
        assert!(matches!(
            ModuleResolver::for_root(&main),
            Err(ResolveModuleError::Manifest(message))
                if message.contains("telora-crate.json") && message.contains("sequence")
        ));

        std::fs::write(app.join(crate::package::CRATE_FILE), "{").unwrap();
        assert!(matches!(
            ModuleResolver::for_root(&main),
            Err(ResolveModuleError::Manifest(message))
                if message.contains("invalid") && message.contains("telora-crate.json")
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
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::create_dir_all(dependency.join("src")).unwrap();
        std::fs::write(app.join("src/main.telora"), "0").unwrap();
        std::fs::write(temporary.join("outside.telora"), "0").unwrap();
        symlink(
            temporary.join("outside.telora"),
            dependency.join("src/escape.telora"),
        )
        .unwrap();
        write_test_workspace(
            &temporary,
            &[("app", "app", &["dep"]), ("dependency", "dep", &[])],
        );
        let resolver = ModuleResolver::for_root(&app.join("src/main.telora")).unwrap();
        let root = resolver.resolve_root(&app.join("src/main.telora")).unwrap();
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
    fn write_test_workspace(root: &Path, members: &[(&str, &str, &[&str])]) {
        fn collect(root: &Path, directory: &Path, modules: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(directory) else {
                return;
            };
            let mut entries = entries.collect::<Result<Vec<_>, _>>().unwrap();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let path = entry.path();
                let file_type = entry.file_type().unwrap();
                if file_type.is_dir() {
                    collect(root, &path, modules);
                    continue;
                }
                if !file_type.is_file() {
                    continue;
                }
                let Ok(format) = ModuleFormat::from_path(&path) else {
                    continue;
                };
                if canonical_path_for_physical(path.strip_prefix(root).unwrap()).is_err() {
                    continue;
                }
                let mut logical = path.strip_prefix(root).unwrap().to_owned();
                if format == ModuleFormat::Telora {
                    logical.set_extension("");
                }
                modules.push(format!(
                    "@src/{}",
                    logical.to_string_lossy().replace('\\', "/")
                ));
            }
        }

        let mut member_paths = Vec::new();
        for (relative, name, dependencies) in members {
            let crate_root = root.join(relative);
            let mut modules = Vec::new();
            collect(&crate_root.join("src"), &crate_root.join("src"), &mut modules);
            modules.sort();
            let mut dependencies = dependencies.to_vec();
            dependencies.sort();
            std::fs::write(
                crate_root.join(crate::package::CRATE_FILE),
                serde_json::to_vec(&serde_json::json!({
                    "name": name,
                    "modules": modules,
                    "dependencies": dependencies,
                }))
                .unwrap(),
            )
            .unwrap();
            member_paths.push(*relative);
        }
        std::fs::write(
            root.join(crate::package::CONFIG_FILE),
            serde_json::to_vec(&serde_json::json!({
                "version": 1,
                "members": member_paths,
            }))
            .unwrap(),
        )
        .unwrap();
        let spec = crate::package::WorkspaceSpec::discover(root).unwrap();
        let lock = spec
            .generate_lock(&std::collections::BTreeMap::new())
            .unwrap();
        spec.write_lock(&lock).unwrap();
    }
