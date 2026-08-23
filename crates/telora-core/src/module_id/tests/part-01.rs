    #[test]
    fn formats_logical_ids_without_physical_paths() {
        let id = ModuleCName::Source(PathBuf::from("a file.yml"));
        assert_eq!(id.to_string(), "@src/a file.yml");
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
        std::fs::create_dir_all(dependency.join("tests")).unwrap();
        std::fs::write(app.join("src/lib.telora"), "0").unwrap();
        std::fs::write(app.join("src/rules.priv.telora"), "0").unwrap();
        std::fs::write(app.join("src/codec.native.telora"), "0").unwrap();
        std::fs::write(app.join("src/serve.entry.telora"), "0").unwrap();
        std::fs::write(app.join("src/schema"), "{}").unwrap();
        std::fs::write(app.join("src/ignored.txt"), "ignored").unwrap();
        std::fs::write(app.join("src/bin/tool.telora"), "0").unwrap();
        std::fs::write(app.join("tests/query.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/public.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/internal.priv.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/system.native.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/bin/tool.telora"), "0").unwrap();
        std::fs::write(dependency.join("tests/query.telora"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"dependencies":{"dep":{"path":"../dependency"}},"formats":{"@src/schema":"json"}}"#,
        )
        .unwrap();

        let catalog = ModuleResolver::catalog_from_cwd(
            &app,
            [
                ("std/string".to_owned(), 1),
                ("std/rt.priv.telora".to_owned(), 2),
                ("std/host.native.telora".to_owned(), 3),
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
                "@bin/tool.telora",
                "@src/codec.native.telora",
                "@src/lib.telora",
                "@src/rules.priv.telora",
                "@src/schema",
                "@src/serve.entry.telora",
                "@test/query.telora",
                "dep/public.telora",
                "std/string",
            ]
        );
        assert_eq!(
            by_name["@src/codec.native.telora"].visibility,
            ModuleVisibility::Native
        );
        assert_eq!(
            by_name["@src/rules.priv.telora"].visibility,
            ModuleVisibility::Private
        );
        assert_eq!(
            by_name["@src/serve.entry.telora"].visibility,
            ModuleVisibility::Entry
        );
        assert_eq!(
            by_name["dep/public.telora"].origin,
            ModuleCatalogOrigin::Dependency
        );
        assert_eq!(by_name["std/string"].origin, ModuleCatalogOrigin::Host);
        assert_eq!(by_name["@src/schema"].format, ModuleFormat::Json);
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
            r#"{"dependencies":{"models":{"path":"models"}}}"#,
        )
        .unwrap();
        std::fs::write(temporary.join("models/user.telora"), "0").unwrap();
        let resolver = ModuleResolver::for_root(&temporary.join("app/main.telora")).unwrap();
        let root = resolver
            .resolve_root(&temporary.join("app/main.telora"))
            .unwrap();
        let dependency = resolver
            .resolve_import(&root.id, "models/user.telora")
            .unwrap();
        assert_eq!(dependency.id.to_string(), "models/user.telora");
        assert_eq!(dependency.format, ModuleFormat::Telora);
        assert!(matches!(
            resolver.resolve_import(&root.id, ""),
            Err(ResolveModuleError::EmptyPath)
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }

    #[test]
    fn builtins_precede_vtops_and_private_modules_stay_crate_local() {
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
        let main = app.join("main.telora");
        std::fs::write(&main, "0").unwrap();
        std::fs::write(app.join("src/local.priv.telora"), "0").unwrap();
        std::fs::write(app.join("src/host.native.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/public.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/internal.priv.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/service.native.telora"), "0").unwrap();
        std::fs::write(dependency.join("src/value.priv.json"), "0").unwrap();
        std::fs::write(dependency.join("src/bad.native.json"), "0").unwrap();
        std::fs::write(shadow.join("src/array"), "0").unwrap();
        std::fs::write(
            app.join("telora-deps.json"),
            r#"{"dependencies":{"dep":{"path":"../dependency"},"std":{"path":"../shadow"}},"formats":{"std/array":"telora","dep/bad.native.json":"json"}}"#,
        )
        .unwrap();

        let entry = ModuleCName::builtin("std/entry/default.entry.telora");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_builtins([
                ("std/array".to_owned(), 5),
                ("dep/service.native.telora".to_owned(), 1_500),
            ])
            .with_entry_context(entry.clone(), std::iter::empty());
        let root = resolver.resolve_root(&main).unwrap();
        let builtin = resolver.resolve_import(&root.id, "std/array").unwrap();
        assert_eq!(builtin.id, ModuleCName::builtin("std/array"));
        assert_eq!(builtin.authority, ModuleAuthority::RuntimeSystem);

        let local = resolver
            .resolve_import(&root.id, "@src/local.priv.telora")
            .unwrap();
        assert_eq!(local.authority, ModuleAuthority::Ordinary);
        let native = resolver
            .resolve_import(&root.id, "@src/host.native.telora")
            .unwrap();
        assert_eq!(native.authority, ModuleAuthority::PackageSystem);

        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/internal.priv.telora"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/value.priv.json"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        let entry_private = resolver
            .resolve_import(&entry, "dep/internal.priv.telora")
            .unwrap();
        assert_eq!(
            entry_private.id,
            ModuleCName::Dependency {
                name: "dep".into(),
                path: "internal.priv.telora".into(),
            }
        );
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/service.native.telora"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        let entry_native = resolver
            .resolve_import(&entry, "dep/service.native.telora")
            .unwrap();
        assert_eq!(entry_native.authority, ModuleAuthority::RuntimeSystem);
        assert_eq!(entry_native.to_string(), "dep/service.native.telora");
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/bad.native.json"),
            Err(ResolveModuleError::InvalidModuleSuffix(_))
        ));
        let dependency_module = resolver
            .resolve_import(&root.id, "dep/public.telora")
            .unwrap();
        assert!(matches!(
            resolver.resolve_import(&dependency_module.id, "@unknown"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        let internal = resolver
            .resolve_import(&dependency_module.id, "./internal.priv.telora")
            .unwrap();
        assert_eq!(internal.authority, ModuleAuthority::Ordinary);
        assert!(matches!(
            ModuleResolver::for_root(&app.join("src/local.priv.telora"))
                .and_then(|resolver| resolver.resolve_root(&app.join("src/local.priv.telora"))),
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
            r#"{"dependencies":{"parser":{"path":"../dependency"}}}"#,
        )
        .unwrap();

        let entry = ModuleCName::builtin("std/entry/default.entry.telora");
        let resolver = ModuleResolver::for_root(&main)
            .unwrap()
            .with_entry_context(entry.clone(), ["std/rt.priv.telora".to_owned()]);
        let root = resolver.resolve_root(&main).unwrap();
        assert_eq!(root.id, ModuleCName::Binary(PathBuf::from("tool.telora")));
        assert_eq!(root.to_string(), "@bin/tool.telora");

        let local = resolver
            .resolve_import(&root.id, "@src/model/a.telora")
            .unwrap();
        assert_eq!(local.to_string(), "@src/model/a.telora");
        assert!(matches!(
            resolver.resolve_import(&root.id, "./model/a.telora"),
            Err(ResolveModuleError::InvalidImport(message))
                if message.contains("binary and test roots must import crate sources with @src/...")
        ));

        let dependency = resolver
            .resolve_import(&root.id, "parser/model/a.telora")
            .unwrap();
        assert_eq!(dependency.to_string(), "parser/model/a.telora");
        assert_eq!(
            resolver
                .resolve_import(&dependency.id, "@src/model/a.telora")
                .unwrap()
                .id,
            dependency.id
        );
        assert!(matches!(
            resolver.resolve_import(&local.id, "@bin/tool.telora"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
        assert_eq!(entry.to_string(), "std/entry/default.entry.telora");
        assert_eq!(
            resolver
                .resolve_import(&entry, "@bin/tool.telora")
                .unwrap()
                .id,
            ModuleCName::Binary(PathBuf::from("tool.telora"))
        );
        assert_eq!(
            resolver
                .resolve_import(&entry, "std/rt.priv.telora")
                .unwrap()
                .id,
            ModuleCName::builtin("std/rt.priv.telora")
        );
        assert!(matches!(
            resolver.resolve_import(&root.id, "std/rt.priv.telora"),
            Err(ResolveModuleError::PrivateModuleAccess(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&root.id, "@entry"),
            Err(ResolveModuleError::InvalidImport(_))
        ));
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
        std::fs::write(app.join("telora-deps.json"), "{}").unwrap();
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
            ModuleResolver::from_cwd(&app, "@bin/escape.telora"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        assert!(matches!(
            ModuleResolver::from_cwd(&app, "@test/escape.telora"),
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
    fn json_manifest_validates_shape_and_exact_format_overrides() {
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
        std::fs::write(dependency.join("schema"), "{}").unwrap();
        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{
                "dependencies": {"dep": {"path": "dependency"}},
                "formats": {"dep/schema": "json"}
            }"#,
        )
        .unwrap();
        let resolver = ModuleResolver::for_root(&main).unwrap();
        let root = resolver.resolve_root(&main).unwrap();
        let schema = resolver.resolve_import(&root.id, "dep/schema").unwrap();
        assert_eq!(schema.format, ModuleFormat::Json);

        std::fs::write(
            temporary.join("telora-deps.json"),
            r#"{"dependencies": []}"#,
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
            r#"{"dependencies":{"dep":{"path":"dependency"}}}"#,
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
            resolver.resolve_import(&root.id, "dep/../outside.telora"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        assert!(matches!(
            resolver.resolve_import(&root.id, "dep/escape.telora"),
            Err(ResolveModuleError::CrateEscape(_))
        ));
        std::fs::remove_dir_all(temporary).unwrap();
    }
