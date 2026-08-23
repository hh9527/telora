    #[test]
    fn module_skeleton_assigns_separate_stable_function_and_type_slots() {
        let blueprint = module_blueprint(
            "decl call: Fn(Int) -> Int; def call = fn(value) { value }; type Box(T) = struct { value: T }; def other = fn(value) { value }; type State = struct { value: Int }; 0",
        )
        .unwrap();
        let funcs = blueprint
            .slots
            .iter()
            .filter(|slot| slot.kind == StaticSlotKind::Func)
            .collect::<Vec<_>>();
        let types = blueprint
            .slots
            .iter()
            .filter(|slot| slot.kind == StaticSlotKind::TypeConstructor)
            .collect::<Vec<_>>();

        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].name, "call");
        assert_eq!(funcs[0].local, crate::FIRST_DYNAMIC_MODULE_LOCAL);
        assert_eq!((funcs[0].declarations, funcs[0].definitions), (1, 1));
        assert_eq!(funcs[1].name, "other");
        assert_eq!(funcs[1].local, crate::FIRST_DYNAMIC_MODULE_LOCAL + 1);

        assert_eq!(types.len(), 2);
        assert_eq!(types[0].name, "Box");
        assert_eq!(types[0].local, crate::FIRST_DYNAMIC_MODULE_LOCAL);
        assert_eq!(types[1].name, "State");
        assert_eq!(types[1].local, crate::FIRST_DYNAMIC_MODULE_LOCAL + 1);
    }

    #[test]
    fn module_skeleton_rejects_incomplete_or_duplicate_function_slots() {
        let missing = module_blueprint("decl call: Fn(Int) -> Int; 0").unwrap_err();
        assert!(missing.contains("has no definition"));

        let duplicate = module_blueprint(
            "decl call: Fn(Int) -> Int; def call = fn(value) { value }; def call = fn(value) { value }; 0",
        )
        .unwrap_err();
        assert!(duplicate.contains("cannot shadow"));
    }

    #[test]
    fn module_skeleton_allows_only_let_to_shadow_a_definition() {
        module_blueprint("def call = fn(value) { value }; let call = 1; call")
            .expect("let may shadow a definition");

        let direct = module_blueprint(
            "let call = fn(value) { value }; def call = fn(value) { value }; call",
        )
        .unwrap_err();
        assert!(direct.contains("cannot shadow"));

        let through_let = module_blueprint(
            "decl call: Fn(Int) -> Int; let call = fn(value) { value }; def call = fn(value) { value }; call",
        )
        .unwrap_err();
        assert!(through_let.contains("cannot shadow"));
    }

    #[test]
    fn module_skeleton_rejects_explicit_import_name_collisions() {
        for binding in [
            "type Item = struct {value: Int};",
            "def Item = 1;",
            "decl Item: Fn() -> Int;",
            "native Item: Fn() -> Int;",
            "native type Item @7;",
        ] {
            let source =
                format!("import \"./provider.telora\" {{ Item }}; {binding} export {{Item}};");
            let mut sources = SourceDatabase::default();
            let source_id = sources.add("@test/conflict.telora", source);
            let parsed = parse_registered(&sources, source_id);
            let program = parsed.program.unwrap_or_else(|| {
                panic!("conflict fixture did not parse: {source_id:?}: {binding}")
            });
            let diagnostics = module_binding_diagnostics(&program);
            assert!(
                diagnostics.iter().any(|diagnostic| diagnostic
                    .message
                    .contains("conflicts with an earlier explicit binding")),
                "{diagnostics:?}"
            );
            assert_eq!(diagnostics[0].labels.len(), 2);
        }
    }

    fn named_output(value: &crate::ExecutionWorld) -> crate::ValueRef<'_> {
        value.value().dict_get("output").expect("output export")
    }
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_native_callback(
        _: &mut crate::CallContext<'_, '_>,
    ) -> Result<(), crate::NativeError> {
        Ok(())
    }

    fn fixture_answer_callback(
        context: &mut crate::CallContext<'_, '_>,
    ) -> Result<(), crate::NativeError> {
        context.set_int(context.result(), 42)
    }

    fn fixture_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("telora-module-test-{unique}"));
        fs::create_dir(&path).unwrap();
        path
    }

    fn recovery_engine() -> Engine {
        Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(1_000_000),
            session_quota: Quota::with_fuel(1_000_000),
            data_limits: DataLimits::default(),
        })
    }

    #[derive(Default)]
    struct CapturingDebugSink {
        events: Mutex<Vec<crate::DebugEvent>>,
    }

    impl crate::DebugSink for CapturingDebugSink {
        fn emit(&self, event: crate::DebugEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[test]
    fn core_native_module_ids_are_reserved_unique_and_order_independent() {
        let specs = module_specs();
        let identities = specs
            .iter()
            .map(|spec| (spec.name, spec.native_id))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(identities.len(), specs.len());
        assert!(
            identities
                .values()
                .all(|id| *id > 0 && *id <= crate::value::RESERVED_NATIVE_MODULE_MAX)
        );
        assert_eq!(
            identities.values().copied().collect::<HashSet<_>>().len(),
            specs.len()
        );
        assert_eq!(identities.get(crate::core::EXEC_MODULE), Some(&21));
        assert!(!identities.values().any(|id| *id == 12));
        let reordered = specs
            .iter()
            .rev()
            .map(|spec| (spec.name, spec.native_id))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(identities, reordered);
    }

    #[test]
    fn core_prelude_exposes_native_decorator_union_and_validate() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "core/prelude" as prelude;
import "core/prelude" { validate as check };
import "std/result" as result;
type User = struct {name: String};
let user: User = {name: result.unwrap(check(String, "telora"))};
(user, union == prelude.union, validate == prelude.validate, check == prelude.validate)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "({name: \"telora\"}, 'True, 'True, 'True)"
        );
        fs::write(
            directory.join("missing.telora"),
            "import \"core/prelude\" { missing }; missing",
        )
        .unwrap();
        let missing =
            load_module(directory.join("missing.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(missing.to_string().contains("has no export \"missing\""));

        fs::write(
            directory.join("duplicate.telora"),
            "import \"core/prelude\" { union as item, validate as item }; item",
        )
        .unwrap();
        let duplicate =
            load_module(directory.join("duplicate.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(
            duplicate
                .to_string()
                .contains("module binding \"item\" conflicts with an earlier explicit binding")
        );

        fs::write(
            directory.join("local.telora"),
            r#"import "core/prelude" { validate as builtin_validate };
import "std/result" as result;
type Plan = struct {revision: Int};
type Profile = struct {enabled: Bool};
def default_profile: Profile = {enabled: 'True};
def validate: Fn(Plan, Profile) -> Plan = fn(plan, profile) { plan };
let plan: Plan = {revision: 1};
let checked = validate(plan, default_profile);
let builtin_checked = result.unwrap(builtin_validate(Int, checked.revision));
export def output: Int = checked.revision + builtin_checked;"#,
        )
        .unwrap();
        let local = load_module(directory.join("local.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            named_output(&local.execute(100_000).unwrap()).to_string(),
            "2"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn open_imports_resolve_lazily_and_combine_with_module_bindings() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"import "std/result" as result, *;
import "core/prelude" as prelude, { validate as check };
type User = struct {name: String};
let user = {name: unwrap('Ok("telora"))};
(user, result.unwrap == unwrap, prelude.validate == check)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "({name: \"telora\"}, 'True, 'True)"
        );

        fs::write(
            directory.join("left.telora"),
            "def shared: Int = 1; export { shared };",
        )
        .unwrap();
        fs::write(
            directory.join("right.telora"),
            "def shared: Int = 2; export { shared };",
        )
        .unwrap();
        fs::write(
            directory.join("unused.telora"),
            "import \"./left.telora\" *; import \"./right.telora\" *; 0",
        )
        .unwrap();
        let unused =
            load_module(directory.join("unused.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(unused.execute(100_000).unwrap().to_string(), "0");

        fs::write(
            directory.join("shadowed.telora"),
            "import \"./left.telora\" *; import \"./right.telora\" *; let shared = 3; shared",
        )
        .unwrap();
        let shadowed =
            load_module(directory.join("shadowed.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(shadowed.execute(100_000).unwrap().to_string(), "3");

        fs::write(
            directory.join("ambiguous.telora"),
            "import \"./left.telora\" *; import \"./right.telora\" *; export { shared as output };",
        )
        .unwrap();
        let ambiguous =
            load_module(directory.join("ambiguous.telora"), BTreeMap::new(), 100_000).unwrap_err();
        let message = ambiguous.to_string();
        assert!(
            message.contains("open import name \"shared\" is ambiguous"),
            "{message}"
        );
        assert!(message.contains("left.telora"));
        assert!(message.contains("right.telora"));
        let recovered = recovery_engine()
            .recover_workspace(directory.join("ambiguous.telora"))
            .unwrap();
        assert!(recovered.diagnostics().iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("open import name \"shared\" is ambiguous")
        }));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn explicit_exports_synthesize_typed_identity_preserving_module_records() {
        let directory = fixture_dir();
        fs::write(
            directory.join("library.telora"),
            r#"let private = "hidden";
export def identity: for(A) Fn(A) -> A = fn(value) { value };
export def answer = 42;
export { identity as map };"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./library.telora" as library, { identity as id, answer };
import "./library.telora" *;
(id(1), id("telora"), answer, map == library.map, library.identity == library.map)"#,
        )
        .unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        assert_eq!(
            module.execute(100_000).unwrap().to_string(),
            "(1, \"telora\", 42, 'True, 'True)"
        );
        let snapshot = recovery_engine()
            .recover_workspace(directory.join("main.telora"))
            .unwrap();
        let library = snapshot
            .module_by_path(&canonicalize(&directory.join("library.telora")).unwrap())
            .unwrap();
        let exports = snapshot
            .exports_of(library.id)
            .into_iter()
            .map(|export| export.name)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            exports,
            BTreeSet::from(["answer".into(), "identity".into(), "map".into()])
        );

        fs::write(
            directory.join("private.telora"),
            "import \"./library.telora\" { private }; private",
        )
        .unwrap();
        let private =
            load_module(directory.join("private.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(private.to_string().contains("has no export \"private\""));

        fs::write(
            directory.join("forward.telora"),
            "export { later }; let later = 1;",
        )
        .unwrap();
        let forward =
            load_module(directory.join("forward.telora"), BTreeMap::new(), 100_000).unwrap_err();
        assert!(
            forward
                .to_string()
                .contains("cannot export unknown or forward binding \"later\"")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn production_modules_require_explicit_exports() {
        let directory = fixture_dir();
        let module = directory.join("missing-export.telora");
        fs::write(&module, "def value = 42;").unwrap();

        let engine = recovery_engine();
        let error = engine.load_module(&module, BTreeMap::new()).unwrap_err();
        assert!(
            error
                .message()
                .contains("requires at least one explicit export"),
            "{}",
            error.message()
        );

        let snapshot = engine.recover_workspace(&module).unwrap();
        let module = snapshot
            .module_by_path(&canonicalize(&module).unwrap())
            .unwrap();
        assert_eq!(module.state, WorkspaceModuleState::Available);
        assert_eq!(
            snapshot
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic
                    .message
                    .contains("requires at least one explicit export"))
                .count(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn production_modules_reject_lexical_and_expression_top_levels() {
        let directory = fixture_dir();
        let engine = recovery_engine();

        for (name, source, expected) in [
            (
                "let.telora",
                "let value = 42; export {value};",
                "module-level let is not supported",
            ),
            (
                "result.telora",
                "def value = 42; value",
                "top-level expressions are not supported",
            ),
            (
                "export-let.telora",
                "export let value = 42;",
                "export let is not supported",
            ),
        ] {
            let path = directory.join(name);
            fs::write(&path, source).unwrap();
            let error = engine.load_module(&path, BTreeMap::new()).unwrap_err();
            assert!(error.message().contains(expected), "{}", error.message());
        }

        fs::write(
            directory.join("valid.telora"),
            "def value: Int = do { let base = 40; base + 2 }; export {value};",
        )
        .unwrap();
        engine
            .load_module(directory.join("valid.telora"), BTreeMap::new())
            .unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn imported_type_family_collision_is_sourced_and_recoverable() {
        let directory = fixture_dir();
        let provider = directory.join("provider.telora");
        let main = directory.join("main.telora");
        fs::write(&provider, "export type Capability(T) = enum { 'Value(T) };").unwrap();
        fs::write(
            &main,
            r#"import "./provider.telora" { Capability };
type Capability = enum { 'Local };
export {Capability};"#,
        )
        .unwrap();

        let engine = recovery_engine();
        let error = engine.load_module(&main, BTreeMap::new()).unwrap_err();
        assert!(
            error
                .message()
                .contains("module binding \"Capability\" conflicts"),
            "{}",
            error.message()
        );
        assert!(
            error.message().contains("first bound here"),
            "{}",
            error.message()
        );

        let snapshot = engine.recover_workspace(&main).unwrap();
        let diagnostic = snapshot
            .diagnostics()
            .iter()
            .find(|diagnostic| {
                diagnostic
                    .message
                    .contains("binding \"Capability\" conflicts")
            })
            .expect("recovery keeps the binding conflict");
        assert_eq!(diagnostic.labels.len(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn declared_enum_context_reaches_branches_closures_and_nested_literals() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"type Expr = enum { 'All, 'Column(String), 'Scalar(Int) };
type Operator = enum { 'Filter(Expr), 'Project(Array(Expr)) };
type Val = enum { 'Int(Int), 'Float(Float) };
type Plan = struct {expr: Expr, operators: Array(Operator), values: Array(Val)};

def branch: Expr = if 'True { 'All } else { 'Column("fallback") };
def make: Fn() -> Expr = fn() { 'Column("id") };

export def output: Plan = do {
    let local_branch: Expr = if 'True { 'Scalar(1) } else { 'All };
    let local_make: Fn() -> Expr = fn() { 'Column("name") };
    let forward: Array(Expr) = ['All, 'Column("first")];
    let reverse: Array(Expr) = ['Column("second"), 'All];
    let operators: Array(Operator) = [
        'Filter(local_branch),
        'Project(forward),
        'Project(reverse),
    ];
    let values: Array(Val) = ['Int(1), 'Float(2.0)];
    {expr: if 'True { local_make() } else { make() }, operators, values}
};"#,
        )
        .unwrap();

        recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap();

        fs::write(
            &main,
            r#"type Expr = enum { 'Column(String) };
def raw = 'Column("id");
export def output: Expr = raw;"#,
        )
        .unwrap();
        let frozen = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(
            frozen.message().contains("inferred as narrower variant")
                && frozen.message().contains("expected Expr"),
            "{}",
            frozen.message()
        );

        fs::write(
            &main,
            r#"type Expr = enum { 'Column(String) };
export def output: Expr = 'Missing;"#,
        )
        .unwrap();
        let illegal = recovery_engine()
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(
            illegal.message().contains("Missing"),
            "{}",
            illegal.message()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn declared_enum_failures_are_named_sourced_and_actionable() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let recover = |source: &str, expected: &str| {
            fs::write(&main, source).unwrap();
            let snapshot = recovery_engine().recover_workspace(&main).unwrap();
            snapshot
                .diagnostics()
                .iter()
                .find(|diagnostic| diagnostic.message == expected)
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "missing diagnostic {expected:?}; found {:#?}",
                        snapshot.diagnostics()
                    )
                })
        };

        let illegal = r#"type Expr = enum { 'Column(String) };
export def output: Expr = 'Missing;"#;
        let diagnostic = recover(illegal, "variant 'Missing is not part of Expr");
        assert_eq!(diagnostic.labels.len(), 2);
        assert_eq!(
            diagnostic.labels[0].location.range(),
            illegal.find("'Missing").unwrap()..illegal.find("'Missing").unwrap() + "'Missing".len()
        );
        let expected = illegal.rfind("Expr").unwrap();
        assert_eq!(
            diagnostic.labels[1].location.range(),
            expected..expected + "Expr".len()
        );
        assert!(diagnostic.notes.is_empty());
        assert!(!diagnostic.message.contains("enum {"));

        let narrow = r#"type Expr = enum { 'All, 'Column(String) };
def raw = 'Column("id");
export def output: Expr = raw;"#;
        let diagnostic = recover(
            narrow,
            "value was inferred as narrower variant 'Column; expected Expr",
        );
        assert_eq!(diagnostic.labels.len(), 2);
        let origin = narrow.find("'Column(\"id\")").unwrap();
        assert_eq!(
            diagnostic.labels[0].location.range(),
            origin..origin + "'Column(\"id\")".len()
        );
        assert_eq!(
            diagnostic.notes,
            vec!["consider annotating the direct definition or collection as Expr".to_owned()]
        );

        let collection = r#"type Expr = enum { 'All, 'Column(String) };
def raw = ['All, 'Column("id")];
export def output: Array(Expr) = raw;"#;
        let diagnostic = recover(
            collection,
            "value was inferred as narrower variants 'All | 'Column; expected Expr",
        );
        let origin = collection.find("['All, 'Column(\"id\")]").unwrap();
        assert_eq!(
            diagnostic.labels[0].location.range(),
            origin..origin + "['All, 'Column(\"id\")]".len()
        );
        assert_eq!(
            diagnostic.notes,
            vec!["consider annotating the direct definition or collection as Expr".to_owned()]
        );

        let payload = r#"type Pair = struct {left: Int, right: Int};
type Expr = enum { 'Compare(Pair) };
export def output: Expr = 'Compare({left: 1, right: "wrong"});"#;
        let diagnostic = recover(
            payload,
            "variant 'Compare payload is incompatible with Expr: field right: cannot unify String with Int",
        );
        assert_eq!(diagnostic.labels.len(), 2);
        let mismatch = payload.find("\"wrong\"").unwrap();
        assert_eq!(
            diagnostic.labels[0].location.range(),
            mismatch..mismatch + "\"wrong\"".len()
        );
        assert!(diagnostic.notes.is_empty());
        assert!(!diagnostic.message.contains("enum {"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn native_type_slots_are_explicit_unique_and_order_independent() {
        fn declarations(
            source: &str,
        ) -> Result<BTreeMap<u32, (String, crate::NativeType)>, ModuleError> {
            let mut sources = SourceDatabase::default();
            let source_id = sources.add("<fixture>", source);
            let program = parse_registered(&sources, source_id).program.unwrap();
            declared_native_types(
                &program,
                crate::value::NativeModuleId(1024),
                "host:fixture",
                &sources,
            )
        }

        let forward = declarations("native type First @7; native type Second @2; First").unwrap();
        let reversed = declarations("native type Second @2; native type First @7; First").unwrap();
        assert_eq!(forward.get(&2).unwrap().1, reversed.get(&2).unwrap().1);
        assert_eq!(forward.get(&7).unwrap().1, reversed.get(&7).unwrap().1);

        let duplicate =
            declarations("native type First @7; native type Second @7; First").unwrap_err();
        assert!(
            duplicate
                .to_string()
                .contains("duplicate native type slot @7")
        );

        let overflow = declarations("native type Huge @4294967296; Huge").unwrap_err();
        assert!(overflow.to_string().contains("must fit the u32 range"));
    }

    #[test]
    fn engine_builder_allocates_and_freezes_host_native_modules() {
        fn spec(name: &str) -> NativeModuleSpec {
            NativeModuleSpec::new(
                name,
                "native type Token @7; native make: Fn() -> Token; {Token: Token, make: make}",
                vec![(
                    "make",
                    crate::NativeFunction::new_with_native_type(
                        "host.make",
                        0,
                        7,
                        fixture_native_callback,
                    ),
                )],
            )
        }

        let mut builder = Engine::builder(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
            data_limits: DataLimits::default(),
        });
        assert_eq!(
            builder
                .register_native_module(Some(2_000), spec("acme/stable"))
                .unwrap(),
            2_000
        );
        assert_eq!(
            builder
                .register_native_module(None, spec("acme/automatic"))
                .unwrap(),
            1_024
        );
        assert!(
            builder
                .register_native_module(Some(2_000), spec("acme/collision"))
                .unwrap_err()
                .to_string()
                .contains("already registered")
        );
        assert!(
            builder
                .register_native_module(Some(2_001), spec("acme/stable"))
                .unwrap_err()
                .to_string()
                .contains("name")
        );
        assert!(
            builder
                .register_native_module(Some(1_023), spec("acme/reserved"))
                .unwrap_err()
                .to_string()
                .contains("reserved range")
        );
        assert!(
            builder
                .register_native_module(None, spec("invalid"))
                .unwrap_err()
                .to_string()
                .contains("absolute module path")
        );
        assert!(
            builder
                .register_native_module(None, spec("std/hash"))
                .unwrap_err()
                .to_string()
                .contains("already registered by Telora")
        );
        assert_eq!(
            builder
                .register_native_module(None, spec("core/future"))
                .unwrap(),
            1_025
        );
        assert_eq!(
            builder
                .register_native_module(None, spec("acme/after-errors"))
                .unwrap(),
            1_026
        );

        let engine = builder.build();
        assert_eq!(
            engine
                .native_modules
                .iter()
                .map(|module| module.id)
                .collect::<Vec<_>>(),
            [1_024, 1_025, 1_026, 2_000]
        );
        let directory = fixture_dir();
        fs::write(directory.join("main.telora"), "export def output = 1;").unwrap();
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        assert_eq!(
            named_output(&engine.execute(&module).unwrap()).to_string(),
            "1"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn registered_host_modules_flow_through_execution_and_workspace_recovery() {
        let config = EngineConfig {
            module_quota: Quota::with_fuel(500_000),
            session_quota: Quota::with_fuel(500_000),
            data_limits: DataLimits::default(),
        };
        let mut builder = Engine::builder(config);
        builder
            .register_native_module(
                Some(1_500),
                NativeModuleSpec::new(
                    "acme/runtime",
                    "native type Token @9; native answer: Fn() -> Int; export { Token, answer };",
                    vec![(
                        "answer",
                        crate::NativeFunction::new(
                            "acme/runtime.answer",
                            0,
                            fixture_answer_callback,
                        ),
                    )],
                ),
            )
            .unwrap();
        let engine = builder.build();
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "acme/runtime" as host;
import "std/type-desc" as desc;
export def output = {answer: host.answer(), name: desc.opaque_name(host.Token)};"#,
        )
        .unwrap();

        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output_world = engine.execute(&module).unwrap();
        let output = named_output(&output_world);
        assert_eq!(output.dict_get("answer").unwrap().to_string(), "42");
        assert_eq!(
            output.dict_get("name").unwrap().to_string(),
            "'Some(\"acme/runtime#Token\")"
        );
        let host = module
            .workspace
            .modules()
            .iter()
            .find(|module| module.name == "acme/runtime")
            .unwrap();
        assert_eq!(host.kind, WorkspaceModuleKind::Core);
        assert_eq!(host.state, WorkspaceModuleState::Available);

        let snapshot = engine.recover_workspace(&main).unwrap();
        assert!(snapshot.diagnostics().is_empty());
        assert!(snapshot.modules().iter().any(|module| {
            module.name == "acme/runtime"
                && module.kind == WorkspaceModuleKind::Core
                && module.state == WorkspaceModuleState::Available
        }));
        let clock = crate::RevisionClock::default();
        let context = crate::QueryContext::current(clock);
        let source = snapshot
            .module_by_path(&fs::canonicalize(&main).unwrap())
            .unwrap()
            .source
            .unwrap();
        let source_text = snapshot.sources().get(source).text().to_string();
        let needle = "host.answer";
        let offset = source_text.find(needle).unwrap() + needle.len();
        let completion = block_on_recovery(snapshot.query_completion_at(
            &context,
            crate::Location::new(source, crate::TextRange::at(offset as u32)),
        ))
        .unwrap()
        .unwrap();
        assert_eq!(completion.candidates.len(), 1);
        assert_eq!(completion.candidates[0].label, "answer");
        assert_eq!(
            completion.candidates[0].kind,
            crate::CompletionKind::ModuleExport
        );
        let async_snapshot =
            block_on_recovery(engine.recover_workspace_async(&main, &BTreeMap::new(), &context))
                .unwrap();
        assert!(async_snapshot.diagnostics().is_empty());

        let isolated = Engine::new(config)
            .load_module(&main, BTreeMap::new())
            .unwrap_err();
        assert!(isolated.to_string().contains("unknown dependency"));
        assert!(isolated.to_string().contains("main.telora:1:8"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn selected_entry_runs_graph_visible_registered_native_modules() {
        let config = EngineConfig {
            module_quota: Quota::with_fuel(500_000),
            session_quota: Quota::with_fuel(500_000),
            data_limits: DataLimits::default(),
        };
        let native_source = "native answer: Fn() -> Int; export { answer };";
        let mut builder = Engine::builder(config);
        builder
            .register_native_module(
                Some(1_500),
                NativeModuleSpec::new(
                    "dep/service.native.telora",
                    native_source,
                    vec![(
                        "answer",
                        crate::NativeFunction::new(
                            "dep/service.answer",
                            0,
                            fixture_answer_callback,
                        ),
                    )],
                ),
            )
            .unwrap();
        let engine = builder.build();
        let directory = fixture_dir();
        fs::create_dir_all(directory.join("src/bin")).unwrap();
        fs::create_dir_all(directory.join("dependency/src")).unwrap();
        fs::write(
            directory.join("telora-deps.json"),
            r#"{"dependencies":{"dep":{"path":"dependency"}}}"#,
        )
        .unwrap();
        fs::write(
            directory.join("dependency/src/service.native.telora"),
            native_source,
        )
        .unwrap();
        fs::write(
            directory.join("src/bin/main.telora"),
            "export def marker = 0;",
        )
        .unwrap();
        let entry = directory.join("src/host.entry.telora");
        fs::write(
            &entry,
            r#"import "std/rt.priv.telora" as rt;
import "dep/service.native.telora" as service;
type Main = struct {marker: Int};
export type MainType = Main;
export type State = Int;
type Reducer = Fn(State, rt.SystemEvent) -> Tuple([State, Array(rt.SystemEffect)]);
type Initializer = Fn(rt.SystemResources, MainType) -> Tuple([State, Reducer]);
export def config:
    Fn(rt.SystemOptions, rt.Env) -> Tuple([rt.SystemCaps, Initializer])
    = fn(options, env) {
    ({data_srcs: {}, spawn_child: 'False, text_srcs: {}, vars: [], stdin: 'Null}, fn(resources: rt.SystemResources, main: MainType) {
        let reduce: Reducer = fn(state, event) {
            match event {
                'Initialize => (state, ['Output("42"), 'Exit(0)]),
                _ => fail!("unexpected event", event),
            }
        };
        (service.answer(), reduce)
    })
};"#,
        )
        .unwrap();
        let pending = engine
            .prepare_module_id(&directory, "@bin/main.telora")
            .unwrap();
        let outcome =
            block_on_recovery(engine.run_pending(pending, "@src/host.entry.telora", &[])).unwrap();
        assert_eq!(outcome.output, "42");
        assert_eq!(outcome.termination, RunTermination::Exit(0));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unknown_host_modules_are_unavailable_without_blocking_independent_facts() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "acme/missing" as missing;
type Independent = String;
{Independent: Independent}"#,
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.message.contains("unknown dependency")
                && diagnostic.labels[0].location.start == 7
        }));
        let root = snapshot
            .module_by_path(&fs::canonicalize(&main).unwrap())
            .unwrap();
        let independent = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.module == root.id && definition.name == "Independent")
            .unwrap();
        assert_eq!(independent.ty.state, crate::FactState::Known);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn contextual_debug_observes_values_with_authored_context() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"def identity: Fn(Any) -> Any = fn(value) { value };
               def data = { text: "line\nnext", items: [1, 'Ok, (2,)] };
               def observed = dbg!(data, "loaded\nvalue");
               def seen_identity = dbg!(identity);
               def seen_value = dbg!(observed);
               def whole_float = dbg!(3.0);
               def negative_zero = dbg!(-0.0);
               export def output = if seen_identity == identity { seen_value } else { data };"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        let engine = Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
            data_limits: DataLimits::default(),
        })
        .with_debug_sink(sink.clone());
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        assert_eq!(
            named_output(&engine.execute(&module).unwrap()).to_string(),
            "{items: [1, 'Ok, (2)], text: \"line\\nnext\"}"
        );
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 5);
        assert_eq!(events[0].message.as_deref(), Some("loaded\nvalue"));
        assert_eq!(events[0].name, "data");
        assert!(events[0].module.ends_with("main.telora"));
        assert_eq!(events[0].line, 3);
        assert_eq!(
            events[0].repr,
            "{items: [1, 'Ok, (2)], text: \"line\\nnext\"}"
        );
        assert_eq!(events[1].name, "identity");
        assert!(events[1].repr.starts_with("<fn-ref "));
        assert_eq!(events[2].name, "observed");
        assert_eq!(events[2].repr, events[0].repr);
        assert_eq!(events[3].name, "3.0");
        assert_eq!(events[3].repr, "3.0");
        assert_eq!(events[4].name, "-0.0");
        assert_eq!(events[4].repr, "-0.0");
        drop(events);

        fs::write(
            directory.join("bad-message.telora"),
            r#"def message = "dynamic"; export def output = dbg!(42, message);"#,
        )
        .unwrap();
        let bad = engine
            .load_module(directory.join("bad-message.telora"), BTreeMap::new())
            .unwrap_err();
        assert!(bad.to_string().contains("String literal"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn runtime_debug_does_not_emit_during_bootstrap_analysis() {
        let directory = fixture_dir();
        let sink = Arc::new(CapturingDebugSink::default());
        let engine = Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
            data_limits: DataLimits::default(),
        })
        .with_debug_sink(sink.clone());

        for (name, type_binding) in [
            ("without-type.telora", ""),
            ("with-type.telora", "type Number = Int;"),
        ] {
            let path = directory.join(name);
            fs::write(
                &path,
                format!(
                    "{type_binding}\ndef value = 1;\ndef observed = dbg!(value);\nexport def output = \"ok\";"
                ),
            )
            .unwrap();
            let before = sink.events.lock().unwrap().len();
            let module = engine.load_module(path, BTreeMap::new()).unwrap();
            assert_eq!(sink.events.lock().unwrap().len(), before, "{name}");
            assert_eq!(
                named_output(&engine.execute(&module).unwrap()).to_string(),
                "\"ok\""
            );
            assert_eq!(sink.events.lock().unwrap().len(), before + 1, "{name}");
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn contextual_debug_is_outside_telora_fuel_and_allocation() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"export def output = dbg!(42, "answer");"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        let engine = Engine::new(EngineConfig {
            module_quota: Quota::with_fuel(100_000),
            session_quota: Quota::with_fuel(100_000),
            data_limits: DataLimits::default(),
        })
        .with_debug_sink(sink.clone());
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        let initial_events = sink.events.lock().unwrap().len();
        let mut exact = QuotaAccount::new(Quota::new(0, 1_000, u64::MAX));
        let arena = Vm::new()
            .with_debug_sink(sink.clone())
            .execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut exact,
            )
            .unwrap();
        let output = crate::ExecutionWorld::new(Arc::clone(&module.runtime.main.heap), arena);
        assert_eq!(named_output(&output).to_string(), "42");
        assert_eq!(sink.events.lock().unwrap().len(), initial_events + 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn metadata_only_helpers_are_erased_but_runtime_helpers_are_retained() {
        let directory = fixture_dir();
        fs::write(
            directory.join("erased.telora"),
            r#"def observe: Fn(Any) -> Any = fn(value) { dbg!(value, "metadata") };
               type Observed = observe(Int);
               0"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        let erased = load_module_with_quota_and_debug_sink(
            directory.join("erased.telora"),
            BTreeMap::new(),
            Quota::with_fuel(100_000),
            sink.clone(),
        )
        .unwrap();
        assert_eq!(sink.events.lock().unwrap().len(), 1);
        assert_eq!(
            erased
                .execute_with_quota(Quota::new(0, 1_000, 0))
                .unwrap()
                .to_string(),
            "0"
        );
        assert_eq!(sink.events.lock().unwrap().len(), 1);

        fs::write(
            directory.join("retained.telora"),
            r#"def observe: Fn(Any) -> Any = fn(value) { dbg!(value, "observed") };
               type Observed = observe(Int);
               observe(1)"#,
        )
        .unwrap();
        let retained = load_module_with_quota_and_debug_sink(
            directory.join("retained.telora"),
            BTreeMap::new(),
            Quota::with_fuel(100_000),
            sink.clone(),
        )
        .unwrap();
        assert_eq!(sink.events.lock().unwrap().len(), 2);
        retained
            .execute_with_quota_and_debug_sink(Quota::with_fuel(2), sink.clone())
            .unwrap();
        assert_eq!(sink.events.lock().unwrap().len(), 3);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bootstrap_shadow_does_not_consume_the_module_initialization_quota() {
        let directory = fixture_dir();
        fs::write(
            directory.join("main.telora"),
            r#"type Observed = dbg!(Int);
               0"#,
        )
        .unwrap();
        let sink = Arc::new(CapturingDebugSink::default());
        load_module_with_quota_and_debug_sink(
            directory.join("main.telora"),
            BTreeMap::new(),
            Quota::new(1, 1_000, u64::MAX),
            sink.clone(),
        )
        .unwrap();
        assert_eq!(
            sink.events.lock().unwrap().len(),
            1,
            "only authoritative MetadataInit is observable and charged"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn derived_codec_normalizes_options_and_pretty_prints_json() {
        let directory = fixture_dir();
        fs::write(
            directory.join("User.telora"),
            r#"import "std/codec" as codec;
               import "std/result" as result;
               import "std/value" { Value };
               type User = struct {v: Option(String)};
               let decode = fn(value) { codec.decode(User, value) };
               let encode = fn(value) {
                   codec.encode(Value, value) |> result.unwrap
               };
               {Type: User, decode: decode, encode: encode}"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./abc.json" { data };
               import "./User.telora" as User;
               import "std/result" as result;
               import "std/json" as json;
               let user = data |> User.decode |> result.unwrap;
               user |> User.encode |> json.stringify_pretty(2)"#,
        )
        .unwrap();

        let expected = [
            (r#"{"v":"abc"}"#, "{\n  \"v\": \"abc\"\n}"),
            (r#"{"v":null}"#, "{\n  \"v\": null\n}"),
            (r#"{}"#, "{\n  \"v\": null\n}"),
        ];
        for (source, output) in expected {
            fs::write(directory.join("abc.json"), source).unwrap();
            let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000)
                .unwrap_or_else(|error| panic!("failed to load {source}: {error}"));
            assert_eq!(
                module.execute(100_000).unwrap().to_string(),
                format!("{output:?}")
            );
        }

        fs::write(directory.join("abc.json"), r#"{"v":1}"#).unwrap();
        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let failure = module.execute(100_000).unwrap_err();
        assert!(failure.message.contains("$.v"), "{}", failure.message);
        assert!(failure.message.contains("String"), "{}", failure.message);
        let data_location = failure
            .data_location()
            .expect("codec failure must retain the invalid JSON value location");
        assert_eq!(
            module.sources.get(data_location.source).name.as_ref(),
            directory.join("abc.json").display().to_string()
        );
        assert_eq!(
            module
                .sources
                .get(data_location.source)
                .slice(data_location)
                .as_deref(),
            Some("1")
        );
        let rendered = failure.to_string();
        assert!(rendered.contains("abc.json:1:6:"), "{rendered}");
        assert!(
            rendered.contains("contract rule declared here"),
            "{rendered}"
        );
        let rule_location = failure
            .rule_location()
            .expect("codec failure must retain the nominal contract location");
        assert!(
            module
                .sources
                .get(rule_location.source)
                .name
                .ends_with("User.telora")
        );
        assert_eq!(
            module
                .sources
                .get(rule_location.source)
                .slice(rule_location)
                .as_deref(),
            Some("struct")
        );

        fs::write(
            directory.join("inspect.telora"),
            r#"import "./abc.json" { data };
               import "./User.telora" as User;
               data |> User.decode"#,
        )
        .unwrap();
        let inspected = load_module(directory.join("inspect.telora"), BTreeMap::new(), 100_000)
            .unwrap()
            .execute(100_000)
            .unwrap();
        let (tag, payload) = inspected.value().tagged_parts().expect("tagged Result");
        assert_eq!(tag.as_atom().as_deref(), Some("Err"));
        assert!(payload.get("message").is_some());
        assert_eq!(payload.get("data").unwrap().to_string(), "1");
        assert_eq!(payload.get("rule").unwrap().to_string(), "{kind: 'String}");
        fs::remove_dir_all(directory).unwrap();
    }
