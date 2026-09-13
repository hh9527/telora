use telora_core::{
    mir::Mir,
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};

fn graph(source: &str) -> Mir {
    let inventory = ["@src/main", "std/prelude"]
        .into_iter()
        .map(|name| ModuleSpec {
            name: name.into(),
            kind: telora_core::mir::ModuleKind::Source,
            native: static_sources::native_module(name),
            implicit_imports: if name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        Ok(if name == "std/prelude" {
            include_str!("../tests/fixtures/prelude.telora").into()
        } else {
            source.into()
        })
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir
}

fn compile(source: &str) -> Result<Vec<u8>, String> {
    compile_export(source, "answer")
}

fn compile_export(source: &str, name: &str) -> Result<Vec<u8>, String> {
    let mir = graph(source);
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == name)
        .unwrap();
    let executable = mir.seal_export(export).unwrap();
    super::compile_executable(&executable)
}

#[test]
fn persistent_source_positions_and_terminal_initialization_failure() {
    let source = include_str!("../tests/fixtures/arithmetic-errors.telora");
    for name in [
        "add",
        "subtract",
        "multiply",
        "multiply_min",
        "divide_min",
        "negate",
        "divide_zero",
        "remainder_zero",
    ] {
        let bytes = compile_export(source, name).unwrap();
        assert!(
            !bytes
                .windows(source.len())
                .any(|window| window == source.as_bytes())
        );
        let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
        assert!(session.eval().is_err());
        let error = session.initialize().unwrap_err();
        assert!(error.starts_with("@src/main:"), "{error}");
        assert!(
            error.contains(if name.ends_with("zero") {
                "division by zero"
            } else {
                "overflowed"
            }),
            "{name}: {error}"
        );
        assert_eq!(session.initialize().unwrap_err(), error);
        assert_eq!(session.eval().unwrap_err(), error);
    }
    let bytes = compile_export(source, "remainder_min").unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.eval().unwrap(), serde_json::json!(0));
}

#[test]
fn interpreter_fuel_is_shared_across_initialization_calls() {
    let bytes =
        compile("def loop = fn(n: Int) -> Int { loop(n + 1) }; export def answer = loop(0);")
            .unwrap();
    let mut session = crate::session::Session::load(&bytes, 1000).unwrap();
    assert!(
        session
            .initialize()
            .unwrap_err()
            .to_lowercase()
            .contains("fuel")
    );
    assert!(session.initialize().is_err());
}

#[test]
fn sealed_export_runs_without_mir_or_host_imports() {
    // compile() drops the entire source/MIR before the engine sees the bytes.
    let bytes = compile("export def answer = 42;").unwrap();
    assert_eq!(bytes, compile("export def answer = 42;").unwrap());
    let engine = wasmi::Engine::default();
    let module = wasmi::Module::new(&engine, &bytes[..]).unwrap();
    assert_eq!(module.imports().count(), 0);
    let mut store = wasmi::Store::new(&engine, ());
    let linker = wasmi::Linker::new(&engine);
    let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
    let initialize = instance
        .get_typed_func::<(), i32>(&store, "telora_initialize")
        .unwrap();
    assert_eq!(initialize.call(&mut store, ()).unwrap(), 1);
    let entry = instance
        .get_typed_func::<(), i32>(&store, "telora_entry")
        .unwrap();
    let pointer = entry.call(&mut store, ()).unwrap() as usize;
    let memory = instance.get_memory(&store, "memory").unwrap();
    let bytes = memory.data(&store);
    assert_eq!(
        i64::from_le_bytes(bytes[pointer + 16..pointer + 24].try_into().unwrap()),
        42
    );
}

#[test]
fn aggregates_use_closed_layouts_and_classified_heap_tables() {
    let bytes = compile(include_str!("../tests/fixtures/aggregates.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([42, [
        {"label": "短文本", "score": 19},
        {"label": "a longer string stored in the string table", "score": 23}
    ], null])
    );
}

#[test]
fn dictionaries_are_sorted_columns_and_use_binary_search() {
    let bytes = compile(include_str!("../tests/fixtures/dictionaries.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([42, {
            "alpha": 23, "beta": 19, "zebra": 1, "a_very_long_key_name": 7
        }])
    );
}

#[test]
fn typed_input_and_post_initialization_calls_keep_main_ids() {
    let bytes = compile(include_str!("../tests/fixtures/call-input.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let before = session.memory.data(&session.store)
        [crate::abi::TABLE_BASE as usize..crate::abi::STATIC_BASE as usize]
        .to_vec();
    for index in 0..32 {
        let input =
            serde_json::json!({"name": "input with a heap allocated string", "values": [index]});
        let result = session.call(&[input]).unwrap();
        assert_eq!(
            result,
            serde_json::json!({"name": "input with a heap allocated string", "total": 20 + index})
        );
    }
    let after = session.memory.data(&session.store);
    for table in 0..crate::abi::TABLE_COUNT as usize {
        let offset = table * crate::abi::TABLE_BYTES as usize + 12;
        assert_eq!(
            &before[offset..offset + 4],
            &after[crate::abi::TABLE_BASE as usize + offset
                ..crate::abi::TABLE_BASE as usize + offset + 4]
        );
    }
}

#[test]
fn language_functions_and_control_flow() {
    for source in [
        include_str!("../tests/fixtures/functions.telora"),
        include_str!("../tests/fixtures/local-recursion.telora"),
        include_str!("../tests/fixtures/short-circuit.telora"),
    ] {
        let bytes = compile(source).unwrap();
        let engine = wasmi::Engine::default();
        let module = wasmi::Module::new(&engine, &bytes[..]).unwrap();
        let mut store = wasmi::Store::new(&engine, ());
        let instance = wasmi::Linker::new(&engine)
            .instantiate_and_start(&mut store, &module)
            .unwrap();
        let init = instance
            .get_typed_func::<(), i32>(&store, "telora_initialize")
            .unwrap();
        assert_eq!(init.call(&mut store, ()).unwrap(), 1, "{source}");
        let entry = instance
            .get_typed_func::<(), i32>(&store, "telora_entry")
            .unwrap();
        let pointer = entry.call(&mut store, ()).unwrap() as usize;
        assert_ne!(pointer, 0, "{source}");
        assert_eq!(entry.call(&mut store, ()).unwrap() as usize, pointer);
        let memory = instance.get_memory(&store, "memory").unwrap();
        let bytes = memory.data(&store);
        assert_eq!(
            i64::from_le_bytes(bytes[pointer + 16..pointer + 24].try_into().unwrap()),
            42,
            "{source}"
        );
    }
}
