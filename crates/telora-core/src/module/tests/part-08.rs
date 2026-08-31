    #[test]
    fn explicit_diagnostics_collect_without_implicit_host_publication() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        let data = directory.join("project.json");
        fs::write(
            directory.join("explicit-diagnostics.telora"),
            include_str!("../../../../../examples/explicit-diagnostics.telora"),
        )
        .unwrap();
        fs::write(
            &data,
            r#"{"name":"","packages":[{"name":"","version":0},{"name":"ok","version":1}]}"#,
        )
        .unwrap();
        fs::write(
            &main,
            r#"import "./explicit-diagnostics" as validation;
import "std/array" as arrays;
import "std/result" as result;
import "./project.json" { data as project };
def initial: Array(validation.DiagnosticRecord) = [];
import "std/codec" as codec;
def checked_input = codec.decode(validation.Project, project) |> result.unwrap;
def output = match validation.validate_project(checked_input, initial) {
    (checked, diagnostics) => {
        count: arrays.length(diagnostics),
        initial_count: arrays.length(initial),
        unchanged: checked == checked_input,
        messages: arrays.map(diagnostics, fn(item) { item.message }),
    },
};
export { output };"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 500_000).unwrap();
        let output_world = module.execute(500_000).unwrap();
        let output = named_output(&output_world);
        assert_eq!(output.dict_get("count").unwrap().to_string(), "3");
        assert_eq!(output.dict_get("initial_count").unwrap().to_string(), "0");
        assert_eq!(output.dict_get("unchanged").unwrap().to_string(), "'True");
        let messages = output.dict_get("messages").unwrap();
        assert_eq!(
            (0..messages.sequence_len().unwrap())
                .map(|index| messages.sequence_get(index).unwrap().to_string())
                .collect::<Vec<_>>(),
            [
                "\"project name must not be empty\"",
                "\"package name must not be empty\"",
                "\"package version must be positive\"",
            ]
        );

        let snapshot = recovery_engine().recover_workspace(&main).unwrap();
        assert!(snapshot.diagnostics().is_empty());

        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn fail_intrinsic_preserves_data_and_authored_rule_locations() {
        let directory = fixture_dir();
        fs::write(directory.join("user.json"), r#"{"age":42}"#).unwrap();
        let source = r#"import "./user.json" { data as user };
import "std/codec" as codec;
import "std/result" as result;
import "std/dyn" as dyn;
type User = struct {age: Int};
def inspect_i: Fn(Dyn) -> Int = fn(value) {
    match dyn.field(value, "age") {
        'Ok(age) => fail!("age rejected", age),
        'Err(error) => fail!(error.message, error),
    }
};
def inspect: for(A) Fn(TypeOf(A)) -> Fn(A) -> Int = interpreter!(inspect_i);
let checked = codec.decode(User, user) |> result.unwrap;
inspect(User)(checked)"#;
        fs::write(directory.join("main.telora"), source).unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        assert!(error.message.contains("age rejected"));
        let data = error.data_location().expect("blame data location");
        assert_eq!(
            module.sources.get(data.source).name.as_ref(),
            "standalone/user.json"
        );
        assert_eq!(
            module.sources.get(data.source).slice(data).as_deref(),
            Some("42")
        );
        let rule = error.rule_location().expect("blame rule location");
        assert_eq!(
            module.sources.get(rule.source).slice(rule).as_deref(),
            Some("inspect(User)(checked)")
        );
        let implementation = error
            .implementation_rule_location()
            .expect("blame implementation location");
        assert_eq!(
            module
                .sources
                .get(implementation.source)
                .slice(implementation)
                .as_deref(),
            Some("fail!(\"age rejected\", age)")
        );
        let rendered = error.to_string();
        assert!(
            rendered.contains("standalone/main:14:1"),
            "{rendered}"
        );
        assert!(rendered.contains("user.json:1:8"), "{rendered}");
        assert!(
            rendered.contains("standalone/main:8:21"),
            "{rendered}"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fail_rule_boundary_crosses_facade_modules() {
        let directory = fixture_dir();
        fs::write(
            directory.join("provider.telora"),
            r#"def reject: Fn(Int) -> Int = fn(value) { fail!("rejected", value) };
export { reject };"#,
        )
        .unwrap();
        fs::write(
            directory.join("facade.telora"),
            r#"import "./provider" as provider;
export def inspect: Fn(Int) -> Int = fn(value) { provider.reject(value) };"#,
        )
        .unwrap();
        fs::write(
            directory.join("main.telora"),
            r#"import "./facade" as facade;
facade.inspect(7)"#,
        )
        .unwrap();

        let module = load_module(directory.join("main.telora"), BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        let source_text = |location: crate::Loc| {
            module
                .sources
                .get(location.source)
                .slice(location)
                .expect("source slice")
                .into_owned()
        };
        assert_eq!(
            source_text(error.rule_location().expect("rule location")),
            "facade.inspect(7)"
        );
        assert_eq!(
            source_text(
                error
                    .implementation_rule_location()
                    .expect("implementation rule location")
            ),
            "fail!(\"rejected\", value)"
        );
        assert_eq!(
            source_text(error.data_location().expect("data location")),
            "7"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fail_rule_boundary_survives_native_callback_continuations() {
        let directory = fixture_dir();
        let main = directory.join("main.telora");
        fs::write(
            &main,
            r#"import "std/array" as array;
let reject: Fn(Int) -> Int = fn(value) { fail!("rejected", value) };
array.map([1], reject)"#,
        )
        .unwrap();

        let module = load_module(&main, BTreeMap::new(), 100_000).unwrap();
        let error = module.execute(100_000).unwrap_err();
        let source_text = |location: crate::Loc| {
            module
                .sources
                .get(location.source)
                .slice(location)
                .expect("source slice")
                .into_owned()
        };
        assert_eq!(
            source_text(error.rule_location().expect("rule location")),
            "array.map([1], reject)"
        );
        assert_eq!(
            source_text(error.data_location().expect("data location")),
            "1"
        );
        assert_eq!(
            source_text(
                error
                    .implementation_rule_location()
                    .expect("implementation rule location")
            ),
            "fail!(\"rejected\", value)"
        );
        fs::remove_dir_all(directory).unwrap();
    }
