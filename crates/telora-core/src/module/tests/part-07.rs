    #[test]
    fn recoverable_workspace_prefers_complete_analysis() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(&main, "type Item = String; export { Item };").unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        let item = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.module == root.id && definition.name == "Item")
            .unwrap();
        assert_eq!(item.ty.state, crate::FactState::Known);
        assert!(snapshot.diagnostics().is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_publishes_precise_type_family_schemes() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(&main, "type Box(A) = struct {value: A}; export { Box };").unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        let family = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.module == root.id && definition.name == "Box")
            .unwrap();
        assert_eq!(
            family.scheme.as_deref(),
            Some("for(A) Fn(TypeOf(A)) -> TypeOf(Box)")
        );
        assert!(!family.scheme.as_deref().unwrap().contains("Any"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_keeps_an_independent_type_family_scheme() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            "type Box(A) = struct {value: A}; let broken = missing; export { Box };",
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        let family = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.module == root.id && definition.name == "Box")
            .unwrap();
        assert_eq!(family.ty.state, crate::FactState::Known);
        assert_eq!(
            family.scheme.as_deref(),
            Some("for(A) Fn(TypeOf(A)) -> TypeOf(Box)")
        );
        assert!(!family.scheme.as_deref().unwrap().contains("Any"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_publishes_runtime_blame_with_data_and_rule_sources() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let data = directory.join("data.json");
        fs::write(&data, r#"{"name":"Telora"}"#).unwrap();
        fs::write(
            &main,
            r#"import "std/result" as result;
               import "std/codec" as codec;
               import "./data.json" { data };
               type Input = struct {name: String};
               let checked = codec.decode(Input, data) |> result.unwrap;
               let output = fail!("invalid name", checked.name);
               export { output };"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        let diagnostic = snapshot
            .diagnostics()
            .iter()
            .find(|diagnostic| diagnostic.message == "invalid name")
            .expect("runtime blame diagnostic");
        assert_eq!(
            diagnostic.labels.len(),
            2,
            "workspace diagnostics: {:#?}",
            snapshot.diagnostics()
        );
        let data_source = snapshot
            .module_by_path(&canonicalize(&data).unwrap())
            .unwrap()
            .source
            .unwrap();
        assert_eq!(diagnostic.labels[0].location.source, root.source.unwrap());
        assert_eq!(diagnostic.labels[1].location.source, data_source);
        assert!(diagnostic.labels[0].primary);
        assert!(!diagnostic.labels[1].primary);
        assert_eq!(diagnostic.labels[1].message, "subject 1 originated here");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_continues_independent_runtime_bindings_without_cascades() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            "let first = 1 / 0;\n\
             let blocked = first + 1;\n\
             let second = 2 / 0;\n\
             export def output = blocked + second;",
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        let division_errors = snapshot
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.message.contains("division by zero"))
            .collect::<Vec<_>>();
        assert_eq!(division_errors.len(), 2, "{division_errors:#?}");
        assert!(
            division_errors[0].labels[0].location.start
                < division_errors[1].labels[0].location.start
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_preserves_partial_arrays_across_bindings() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
let first = array.map([1, 2], fn(item) {
    if item == 1 { fail!("first", item) } else { item }
});
let second = array.map(first, fn(item) {
    if item == 2 { fail!("second", item) } else { item }
});
export def output = `unexpected \{array.length(second)}`;"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .filter(|message| matches!(*message, "first" | "second"))
            .collect::<Vec<_>>();
        assert_eq!(
            messages,
            ["first", "second"],
            "{:#?}",
            snapshot.diagnostics()
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_local_annotations_do_not_reduce_diagnostic_coverage() {
        for (annotation, prelude) in [
            ("", "1"),
            (": Int", "1"),
            (": Tuple([Int, String])", "(1, \"one\")"),
            (": Pair", "{left: 1, right: \"two\"}"),
            (": Array(Tuple([Int, String]))", "[(1, \"one\")]"),
        ] {
            let directory = fixture_dir();
            let main = directory.join("main.telora");
            fs::write(
                &main,
                format!(
                    r#"import "std/array" as array;
type A = enum {{ 'Bad }};
type B = enum {{ 'Bad }};
type Pair = struct {{ left: Int, right: String }};
def fail_a: Fn(A) -> Int = fn(value) {{ fail!("diagnostic A", value) }};
def fail_b: Fn(B) -> Int = fn(value) {{ fail!("diagnostic B", value) }};
def run_both: Fn(Array(A), Array(B)) -> Int = fn(values_a, values_b) {{
    let pre{annotation} = {prelude};
    let first = array.map(values_a, fail_a);
    let second = array.map(values_b, fail_b);
    array.length(first) + array.length(second)
}};
let result = run_both(['Bad], ['Bad]);
export def output = "unreachable";"#
                ),
            )
            .unwrap();

            let snapshot = recovery_engine().recover_workspace(&main).unwrap();
            let messages = snapshot
                .diagnostics()
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .filter(|message| matches!(*message, "diagnostic A" | "diagnostic B"))
                .collect::<Vec<_>>();
            assert_eq!(messages, ["diagnostic A", "diagnostic B"], "{annotation}");
            fs::remove_dir_all(directory).unwrap();
        }
    }

    #[test]
    fn recoverable_workspace_keeps_strict_recursive_types_after_runtime_failure() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"type CallExpr = struct {args: Array(Expr)};
type BinExpr = struct {left: Expr, right: Expr};
type Expr = enum {'Call(CallExpr), 'Bin(BinExpr), 'Text(String)};
type Plan(A) = struct {root: Expr, value: A};
def render: Fn(Expr) -> String = fn(expr) {
    match expr {
        'Call(call) => render(call.args[0]),
        'Bin(bin) => `\{render(bin.left)}\{render(bin.right)}`,
        'Text(text) => text,
    }
};
def transform: for(A) Fn(Plan(A)) -> String = fn(plan) { render(plan.root) };
def duplicate: Fn(Array(Expr)) -> Array(Expr) = fn(items) { items };
def reject: Fn(Int) -> Expr = fn(value) { fail!("expected failure", value) };
def failed = reject(1);
export def output = "unreachable";"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        let messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert_eq!(messages, ["expected failure"]);
        for name in ["CallExpr", "BinExpr", "Expr", "Plan"] {
            let definition = snapshot
                .definitions()
                .iter()
                .find(|definition| definition.module == root.id && definition.name == name)
                .unwrap();
            assert_eq!(definition.ty.state, crate::FactState::Known, "{name}");
        }

        let dependency = directory.join("dependency.telora");
        fs::rename(&main, &dependency).unwrap();
        fs::write(
            &main,
            "import \"./dependency\" as dependency; export def output = \"root\";",
        )
        .unwrap();
        let dependent = recovery_engine().recover_workspace(&main).unwrap();
        assert!(
            dependent
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "expected failure")
        );
        assert!(
            !dependent
                .diagnostics()
                .iter()
                .any(|diagnostic| { diagnostic.message.contains("cannot be partially evaluated") })
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn workspace_modules_keep_failed_and_healthy_exports_across_imports() {
        let directory = fixture_dir();
        let dependency = directory.join("dependency.telora");
        let healthy = directory.join("healthy.telora");
        let blocked = directory.join("blocked.telora");
        fs::write(
            &dependency,
            r#"export def failed = fail!("dependency failed", 1);
export def healthy = 41;"#,
        )
        .unwrap();
        fs::write(
            &healthy,
            r#"import "./dependency" as dependency;
export def output = dependency.healthy + 1;"#,
        )
        .unwrap();
        fs::write(
            &blocked,
            r#"import "./dependency" as dependency;
export def output = dependency.failed + 1;"#,
        )
        .unwrap();

        for root in [&healthy, &blocked] {
            let snapshot = recovery_engine().recover_workspace(root).unwrap();
            assert_eq!(
                snapshot
                    .diagnostics()
                    .iter()
                    .filter(|diagnostic| diagnostic.message == "dependency failed")
                    .count(),
                1,
                "{:#?}",
                snapshot.diagnostics()
            );
            assert!(!snapshot.diagnostics().iter().any(|diagnostic| {
                diagnostic.message.contains("unknown root")
                    || diagnostic.message.contains("finalization is incomplete")
                    || diagnostic.message.contains("module is unavailable")
                    || diagnostic
                        .message
                        .contains("dependent computation received a failed evaluation node")
            }));
            let module = snapshot
                .module_by_path(&canonicalize(root).unwrap())
                .unwrap();
            let output = snapshot
                .definitions()
                .iter()
                .find(|definition| definition.module == module.id && definition.name == "output")
                .unwrap();
            assert_eq!(output.ty.state, crate::FactState::Known);
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn workspace_failure_ids_remain_stable_across_multiple_modules() {
        let directory = fixture_dir();
        let first = directory.join("first.telora");
        let second = directory.join("second.telora");
        let main = directory.join("main.telora");
        fs::write(&first, r#"export def failed = fail!("first failed", 1);"#).unwrap();
        fs::write(&second, r#"export def failed = fail!("second failed", 2);"#).unwrap();
        fs::write(
            &main,
            r#"import "./first" as first;
import "./second" as second;
export def output = second.failed + 1;"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        for message in ["first failed", "second failed"] {
            assert_eq!(
                snapshot
                    .diagnostics()
                    .iter()
                    .filter(|diagnostic| diagnostic.message == message)
                    .count(),
                1,
                "{:#?}",
                snapshot.diagnostics()
            );
        }
        assert!(!snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.message.contains("unknown root")
                || diagnostic
                    .message
                    .contains("dependent computation received a failed evaluation node")
        }));

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_continues_healthy_array_slots_and_skips_failed_slots() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def first: Fn(Int) -> Int = fn(item) {
    if item == 2 { fail!("two", item) }
    else if item == 4 { fail!("four", item) }
    else { item + 10 }
};
def second: Fn(Int) -> Int = fn(item) {
    if item == 13 { fail!("three", item) } else { item + 100 }
};
export def output = [1, 2, 3, 4]
    |> array.map\(_, first)
    |> array.map\(_, second);"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        let mut messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .filter(|message| matches!(*message, "two" | "three" | "four"))
            .collect::<Vec<_>>();
        messages.sort_unstable();
        assert_eq!(messages, ["four", "three", "two"]);

        let strict = load_module(&main, BTreeMap::new(), 100_000)
            .unwrap()
            .execute(100_000);
        assert!(
            strict.is_err(),
            "strict execution published a partial Array"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_does_not_publish_a_clean_root_after_internal_failure() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def transform: Fn(Int) -> Int = fn(item) {
    if item == 2 { fail!("two", item) } else { item + 10 }
};
export def output = match array.get(array.map([1, 2, 3], transform), 0) {
    'Some(value) => value,
    'None => 0,
};"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        assert_eq!(
            snapshot
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message == "two")
                .count(),
            1
        );

        let strict = load_module(&main, BTreeMap::new(), 100_000)
            .unwrap()
            .execute(100_000);
        assert!(strict.is_err(), "strict execution accepted a failed world");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_retains_direct_failed_children_for_diagnostics() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def transform: Fn(Int) -> Int = fn(item) {
    if item == 2 { fail!("two", item) }
    else if item == 3 { fail!("three", item) }
    else { item }
};
export def output = array.length([1, transform(2), 1 / 0, transform(3), 4]);"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "two")
        );
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "three")
        );
        assert!(
            snapshot
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message.contains("division by zero"))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_filter_continues_predicates_but_fold_stops_after_failed_accumulator() {
        let directory = fixture_dir();
        let filter = directory.join("filter.telora");
        fs::write(
            &filter,
            r#"import "std/array" as array;
export def output = array.filter([1, 2, 3, 4], fn(item) {
    if item == 2 { fail!("filter-two", item) }
    else if item == 4 { fail!("filter-four", item) }
    else { item > 0 }
});"#,
        )
        .unwrap();
        let filtered = recovery_engine().recover_workspace(&filter).unwrap();
        for message in ["filter-two", "filter-four"] {
            assert_eq!(
                filtered
                    .diagnostics()
                    .iter()
                    .filter(|diagnostic| diagnostic.message == message)
                    .count(),
                1
            );
        }

        let fold = directory.join("fold.telora");
        fs::write(
            &fold,
            r#"import "std/array" as array;
export def output = array.fold([1, 2, 3], 0, fn(acc, item) {
    if item == 2 { fail!("fold-stop", item) }
    else if item == 3 { fail!("fold-must-not-run", item) }
    else { acc + item }
});"#,
        )
        .unwrap();
        let folded = recovery_engine().recover_workspace(&fold).unwrap();
        assert!(
            folded
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "fold-stop")
        );
        assert!(
            !folded
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.message == "fold-must-not-run")
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_non_shape_array_operations_propagate_without_type_cascades() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def pieces: Fn(Int) -> Array(Int) = fn(item) {
    if item == 2 { fail!("piece-two", item) } else { [item] }
};
let flattened = array.flat_map([1, 2, 3], pieces);
let concatenated = array.concat([[0], flattened, [4]]);
let independent = array.map([5, 6], fn(item) {
    if item == 6 { fail!("independent-six", item) } else { item }
});
export def output = array.length(concatenated) + array.length(independent);"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert!(messages.contains(&"piece-two"), "{messages:#?}");
        assert!(messages.contains(&"independent-six"), "{messages:#?}");
        assert!(
            !messages.iter().any(|message| {
                message.contains("concat item")
                    || message.contains("flat_map callback")
                    || message.contains("expected Func")
            }),
            "{messages:#?}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_function_failure_matrix_has_no_call_cascade() {
        let directory = fixture_dir();
        let dependency = directory.join("dependency.telora");
        let main = directory.join("main.telora");
        fs::write(
            &dependency,
            r#"type Plan(Revision) = struct { revision: Revision };
def ensure_plan: for(Revision) Fn(Plan(Revision), Plan(Revision)) -> Plan(Revision) = fn(left, right) {
    fail!("cross polymorphic", left)
};
def ensure_int: Fn(Int, Int) -> Int = fn(left, right) {
    fail!("cross monomorphic", left)
};
export { Plan, ensure_plan, ensure_int };"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./dependency" as dependency;
def local_poly: for(Item) Fn(Item, Item) -> Item = fn(left, right) {
    fail!("local polymorphic", left)
};
def local_int: Fn(Int, Int) -> Int = fn(left, right) {
    fail!("local monomorphic", left)
};
let plan: dependency.Plan(Int) = { revision: 1 };
let cross_poly = dependency.ensure_plan(plan, plan);
let cross_mono = dependency.ensure_int(1, 2);
let own_poly = local_poly(plan, plan);
let own_mono = local_int(1, 2);
export def output = (cross_poly, cross_mono, own_poly, own_mono);"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        for expected in [
            "cross polymorphic",
            "cross monomorphic",
            "local polymorphic",
            "local monomorphic",
        ] {
            assert_eq!(
                messages
                    .iter()
                    .filter(|message| **message == expected)
                    .count(),
                1,
                "{messages:#?}"
            );
        }
        assert!(
            !messages.iter().any(|message| {
                message.contains("tag constructor")
                    || message.contains("expected Func")
                    || message.contains("expected Dict")
            }),
            "{messages:#?}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_data_consumers_do_not_observe_failed_children_as_values() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
def reject: Fn(Int) -> Int = fn(item) { fail!("nested", item) };
let failed: Array(Int) = array.map([1], reject);
let compared = failed == [1];
let selected = failed[0] == 1;
export def output = (compared, selected);"#,
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let messages = snapshot
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            messages
                .iter()
                .filter(|message| **message == "nested")
                .count(),
            1,
            "{messages:#?}"
        );
        assert!(
            !messages.iter().any(|message| {
                message.contains("expected") || message.contains("non-exhaustive")
            }),
            "{messages:#?}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_records_panic_and_continues_independent_bindings() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            "let failed = panic!(\"broken\");\nlet independent = 2 + 3;\nexport { failed, independent };",
        )
        .unwrap();

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        assert_eq!(root.state, WorkspaceModuleState::Available);
        assert!(snapshot.diagnostics().iter().any(|diagnostic| {
            diagnostic.message == "broken"
                && diagnostic.labels[0].location.source == root.source.unwrap()
        }));
        assert_eq!(
            snapshot
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message == "broken")
                .count(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_links_json_and_core_values() {
        let directory = fixture_dir();
        let data = directory.join("data.json");
        let model = directory.join("model.telora");
        let main = directory.join("main.telora");
        fs::write(&data, r#"{"kind":"int"}"#).unwrap();
        fs::write(&model, "type Shared = String; export { Shared };").unwrap();
        fs::write(
            &main,
            "import \"./data.json\" { data };\
             import \"./model\" as model;\
             import \"std/attributes\" as attributes;\
             type FromTelora = model.Shared;\
             type FromCore = attributes.strip(String);\
             type Broken = missing(Int);\
             export { FromTelora as output };",
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&main).unwrap())
            .unwrap();
        let fact = |name: &str| {
            &snapshot
                .definitions()
                .iter()
                .find(|definition| definition.module == root.id && definition.name == name)
                .unwrap()
                .ty
        };
        for (name, expected) in [("FromTelora", "String"), ("FromCore", "String")] {
            assert_eq!(
                fact(name).state,
                crate::FactState::Known,
                "{name}: {:#?}",
                snapshot.diagnostics()
            );
            assert_eq!(
                snapshot.types().display(fact(name).value.unwrap()).unwrap(),
                expected
            );
        }
        let data_binding = snapshot
            .definitions()
            .iter()
            .find(|definition| definition.module == root.id && definition.name == "data")
            .expect("static data import binding");
        assert_eq!(data_binding.ty.state, crate::FactState::Known);
        assert_eq!(
            snapshot
                .types()
                .display(data_binding.ty.value.unwrap())
                .unwrap(),
            "Value"
        );
        assert!(snapshot.modules().iter().any(|module| {
            module.kind == WorkspaceModuleKind::Json
                && module.state == WorkspaceModuleState::Available
        }));
        assert!(snapshot.modules().iter().any(|module| {
            module.kind == WorkspaceModuleKind::Core
                && module.state == WorkspaceModuleState::Available
        }));
        assert_eq!(
            snapshot
                .module_by_path(&canonicalize(&model).unwrap())
                .unwrap()
                .state,
            WorkspaceModuleState::Available
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recoverable_workspace_retains_module_cycles_once() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let a = directory.join("a.telora");
        let b = directory.join("b.telora");
        fs::write(&main, "import \"./a\" as a; export { a as output };").unwrap();
        fs::write(
            &a,
            "import \"./b\" as b; type A = String; export { A };",
        )
        .unwrap();
        fs::write(
            &b,
            "import \"./a\" as a; type B = String; export { B };",
        )
        .unwrap();
        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        assert_eq!(
            snapshot
                .modules()
                .iter()
                .filter(|module| module.kind == WorkspaceModuleKind::Telora)
                .filter(|module| module.state == WorkspaceModuleState::Available)
                .count(),
            3
        );
        assert_eq!(
            snapshot
                .diagnostics()
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("module cycle"))
                .count(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn core_json_parses_and_decodes_strings_with_blame_results() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/json" as json;
               import "std/result" as result;
               def parsed = result.unwrap(json.parse("{\"answer\": 42}"));
               def answer = match parsed {
                   'Object(fields) => match fields.answer {
                       'Int(value) => value,
                       _ => 0,
                   },
                   _ => 0,
               };
               export def output = {
                   parsed: parsed,
                   answer: answer,
                   decoded: result.unwrap(json.decode(Int, "42")),
                   failed: json.parse("{")
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(
            output.contains("parsed: 'Object({answer: 'Int(42)})"),
            "{output}"
        );
        assert!(output.contains("answer: 42"), "{output}");
        assert!(output.contains("decoded: 42"), "{output}");
        assert!(output.contains("failed: 'Err("), "{output}");
        assert!(output.contains("data: \"{\""), "{output}");
        assert!(output.contains("rule: 'Json"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn runtime_data_parsers_share_the_recursive_value_contract() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/yaml" as yaml;
               import "std/toml" as toml;
               import "std/result" as result;
               def yaml_value = result.unwrap(yaml.parse("item: !!binary SGk="));
               def toml_value = result.unwrap(toml.parse("when = 2026-08-18"));
               def yaml_ok = match yaml_value {
                   'Object(fields) => match fields.item {
                       'Bytes(_) => 'True,
                       _ => 'False,
                   },
                   _ => 'False,
               };
               def toml_ok = match toml_value {
                   'Object(fields) => match fields.when {
                       'LocalDate("2026-08-18") => 'True,
                       _ => 'False,
                   },
                   _ => 'False,
               };
               export def output = {
                   yaml_ok: yaml_ok,
                   toml_ok: toml_ok,
                   custom_tag: yaml.parse("item: !custom value"),
                   non_finite: toml.parse("value = nan"),
               };"#,
        )
        .unwrap();
        let engine = recovery_engine();
        let module = engine.load_module(&main, BTreeMap::new()).unwrap();
        let output = named_output(&engine.execute(&module).unwrap()).to_string();
        assert!(output.contains("yaml_ok: 'True"), "{output}");
        assert!(output.contains("toml_ok: 'True"), "{output}");
        assert!(output.contains("custom_tag: 'Err("), "{output}");
        assert!(output.contains("custom YAML tags"), "{output}");
        assert!(output.contains("non_finite: 'Err("), "{output}");
        assert!(output.contains("must be finite"), "{output}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn static_data_imports_use_structural_limits_instead_of_vm_allocation_quota() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(directory.join("data.json"), r#"{"items":[1,2]}"#).unwrap();
        fs::write(
            &main,
            r#"import "./data.json" { data }; export def output = data;"#,
        )
        .unwrap();

        let engine_with = |data_limits| {
            Engine::new(EngineConfig {
                module_quota: Quota::with_fuel(1_000_000),
                session_quota: Quota::with_fuel(1_000_000),
                data_limits,
            })
        };
        let file_error = engine_with(DataLimits {
            file_size: 4,
            ..DataLimits::default()
        })
        .load_module(&main, BTreeMap::new())
        .unwrap_err()
        .to_string();
        assert!(file_error.contains("file_size"), "{file_error}");

        let node_error = engine_with(DataLimits {
            nodes: 3,
            ..DataLimits::default()
        })
        .load_module(&main, BTreeMap::new())
        .unwrap_err()
        .to_string();
        assert!(node_error.contains("nodes"), "{node_error}");
        assert!(!node_error.contains("allocation quota"), "{node_error}");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn semantic_value_has_one_recursive_identity_and_static_module_shape() {
        fn assert_value_graph<'a>(
            value: crate::ValueRef<'a>,
            expected: &crate::value::DeclaredTypeId,
        ) {
            let (owner, raw) = value
                .declared_value_parts()
                .expect("every semantic Value node has a nominal witness");
            let (actual, name, _) = owner
                .declared_type_parts()
                .expect("Value witness is a declared Type");
            assert_eq!(name, "Value");
            assert_eq!(actual, expected);
            let Some((tag, payload)) = raw.tagged_parts() else {
                assert!(matches!(
                    raw.as_atom().as_deref(),
                    Some("None" | "True" | "False")
                ));
                return;
            };
            match tag.as_atom().as_deref() {
                Some("Array") => {
                    for index in 0..payload.sequence_len().expect("Value.Array payload") {
                        assert_value_graph(payload.sequence_get(index).unwrap(), expected);
                    }
                }
                Some("Object") => {
                    for child in payload.dict_values().expect("Value.Object payload") {
                        assert_value_graph(child, expected);
                    }
                }
                Some(
                    "Int" | "Float" | "String" | "Bytes" | "LocalDate" | "LocalTime"
                    | "LocalDateTime" | "OffsetDateTime",
                ) => {}
                other => panic!("unexpected Value variant {other:?}"),
            }
        }

        let directory = fixture_dir();
        fs::write(
            directory.join("data.json"),
            r#"{"none":null,"true":true,"false":false,"int":1,"float":1.5,"string":"x","array":[2],"object":{"item":3}}"#,
        )
        .unwrap();
        fs::write(directory.join("data.yaml"), "bytes: !!binary SGk=\n").unwrap();
        fs::write(
            directory.join("data.toml"),
            "date = 2026-08-18\ntime = 12:34:56\nlocal = 2026-08-18T12:34:56\noffset = 2026-08-18T12:34:56+08:00\n",
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./data.json" as json_module;
               import "./data.yaml" as yaml_module;
               import "./data.toml" as toml_module;
               import "./data.json" { data as json_data };
               import "./data.yaml" { data as yaml_data };
               import "./data.toml" { data as toml_data };
               import "std/codec" as codec;
               import "std/result" as result;
               import "std/value" { Value };
               def classify: Fn(Value) -> String = fn(value) {
                   match value {
                       'None => "None",
                       'True => "True",
                       'False => "False",
                       'Int(_) => "Int",
                       'Float(_) => "Float",
                       'String(_) => "String",
                       'Bytes(_) => "Bytes",
                       'Array(_) => "Array",
                       'Object(_) => "Object",
                       'LocalDate(_) => "LocalDate",
                       'LocalTime(_) => "LocalTime",
                       'LocalDateTime(_) => "LocalDateTime",
                       'OffsetDateTime(_) => "OffsetDateTime",
                   }
               };
               def json: Dict(Value) = match json_data {'Object(fields) => fields, _ => {}};
               def yaml: Dict(Value) = match yaml_data {'Object(fields) => fields, _ => {}};
               def toml: Dict(Value) = match toml_data {'Object(fields) => fields, _ => {}};
               def encoded_identity = codec.encode(Value, json_data) |> result.unwrap;
               def decoded_identity = codec.decode(Value, encoded_identity) |> result.unwrap;
               export def output = {
                   json_module,
                   yaml_module,
                   toml_module,
                   identity: decoded_identity == json_data,
                   labels: [
                       classify(json.none), classify(json.true), classify(json.false),
                       classify(json.int), classify(json.float), classify(json.string),
                       classify(yaml.bytes), classify(json.array), classify(json.object),
                       classify(toml.date), classify(toml.time), classify(toml.local),
                       classify(toml.offset),
                   ],
               };"#,
        )
        .unwrap();

        let engine = recovery_engine();
        let module = engine
            .load_module(directory.join("main.telora"), BTreeMap::new())
            .unwrap();
        for name in ["json_data", "yaml_data", "toml_data"] {
            assert_eq!(
                module.analysis.display(module.analysis.binding_types[name]),
                "Value"
            );
        }
        let execution = engine.execute(&module).unwrap();
        let output = named_output(&execution);
        assert_eq!(
            output.get("labels").unwrap().to_string(),
            "[\"None\", \"True\", \"False\", \"Int\", \"Float\", \"String\", \"Bytes\", \"Array\", \"Object\", \"LocalDate\", \"LocalTime\", \"LocalDateTime\", \"OffsetDateTime\"]"
        );
        assert_eq!(output.get("identity").unwrap().to_string(), "'True");
        let mut expected = None;
        for name in ["json_module", "yaml_module", "toml_module"] {
            let namespace = output.get(name).unwrap();
            assert_eq!(namespace.module_fields().unwrap(), vec!["data"]);
            let data = namespace.module_get("data").unwrap();
            let (owner, _) = data.declared_value_parts().unwrap();
            let (id, _, _) = owner.declared_type_parts().unwrap();
            if let Some(expected) = expected {
                assert_eq!(id, expected);
            } else {
                expected = Some(id);
            }
            assert_value_graph(data, id);
        }

        let snapshot = engine
            .recover_workspace(directory.join("main.telora"))
            .unwrap();
        let root = snapshot
            .module_by_path(&canonicalize(&directory.join("main.telora")).unwrap())
            .unwrap();
        for file in ["data.json", "data.yaml", "data.toml"] {
            let module = snapshot
                .module_by_path(&canonicalize(&directory.join(file)).unwrap())
                .unwrap();
            let exports = snapshot.exports_of(module.id);
            assert_eq!(exports.len(), 1, "{file}");
            assert_eq!(exports[0].name, "data", "{file}");
            assert_eq!(
                snapshot.types().display(exports[0].ty).unwrap(),
                "Value",
                "{file}"
            );
        }
        for name in ["json_data", "yaml_data", "toml_data"] {
            let definition = snapshot
                .definitions()
                .iter()
                .find(|definition| definition.module == root.id && definition.name == name)
                .unwrap();
            assert_eq!(definition.ty.state, crate::FactState::Known);
            assert_eq!(
                snapshot
                    .types()
                    .display(definition.ty.value.unwrap())
                    .unwrap(),
                "Value"
            );
        }

        fs::write(directory.join("data.yaml"), "value: [\n").unwrap();
        let recovered = recovery_engine()
            .recover_workspace(directory.join("main.telora"))
            .unwrap();
        let yaml = recovered
            .module_by_path(&canonicalize(&directory.join("data.yaml")).unwrap())
            .unwrap();
        let exports = recovered.exports_of(yaml.id);
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].name, "data");
        assert_eq!(recovered.types().display(exports[0].ty).unwrap(), "Value");
        assert!(!recovered.diagnostics().is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn semantic_value_parsing_and_encoding_charge_complete_wrapper_graphs() {
        fn execute_with_allocation(
            module: &LoadedModule,
            allocation_bytes: u64,
        ) -> (Result<(), crate::RuntimeError>, u64) {
            let mut account = QuotaAccount::new(Quota::new(10, 1_000, allocation_bytes));
            let result = Vm::new().execute_in_work(
                &module.runtime.main.heap,
                &module.runtime.externals,
                &module.function,
                &[],
                &mut account,
            );
            (result.map(|_| ()), account.requested_allocation_bytes())
        }

        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let value_bytes = std::mem::size_of::<Val>() as u64;

        fs::write(&main, r#"import "std/json" as json; json.parse("[1]")"#).unwrap();
        let parsed = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let parse_bytes = 3 + 7 * value_bytes;
        let (result, requested) = execute_with_allocation(&parsed, parse_bytes);
        assert!(result.is_ok());
        assert_eq!(requested, parse_bytes);
        let (result, _) = execute_with_allocation(&parsed, parse_bytes - 1);
        assert_eq!(
            result.expect_err("one byte short must fail").kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        fs::write(
            &main,
            r#"import "std/codec" as codec; codec.encode(codec.Value, value)"#,
        )
        .unwrap();
        let encoded = load_module(
            &main,
            BTreeMap::from([(
                "value".into(),
                parse_json("value.json", r#"{"item":[1]}"#).unwrap(),
            )]),
            100_000,
        )
        .unwrap();
        let encode_bytes = 10 * value_bytes;
        let (result, requested) = execute_with_allocation(&encoded, encode_bytes);
        assert!(result.is_ok());
        assert_eq!(requested, encode_bytes);
        let (result, _) = execute_with_allocation(&encoded, encode_bytes - 1);
        assert_eq!(
            result.expect_err("one byte short must fail").kind,
            crate::RuntimeErrorKind::AllocationQuotaExceeded
        );

        fs::remove_dir_all(directory).unwrap();
    }

