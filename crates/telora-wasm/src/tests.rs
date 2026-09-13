use telora_core::{
    mir::Mir,
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};

#[test]
fn diagnostic_scopes_capture_reports_and_resume_outer_execution() {
    let bytes = compile(include_str!("../tests/fixtures/capture.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let value = session.call(&[]).unwrap();
    assert_eq!(value[0]["Ok"][0], 42);
    assert_eq!(value[0]["Ok"][1][0]["message"], "captured warning");
    assert_eq!(value[0]["Ok"][1][0]["severity"], "Warning");
    assert_eq!(value[1]["Err"].as_array().unwrap().len(), 2);
    assert_eq!(value[1]["Err"][1]["message"], "captured failure");
    assert_eq!(value[1]["Err"][1]["labels"].as_array().unwrap().len(), 2);
    assert_eq!(
        value[1]["Err"][1]["labels"][1]["location"]["source"],
        "@src/main"
    );
    assert_eq!(
        value[1]["Err"][1]["labels"][1]["message"],
        "subject 1 originated here"
    );
    assert_eq!(value[2]["Ok"][0], 7);
    assert_eq!(value[2]["Ok"][1].as_array().unwrap().len(), 2);
    assert_eq!(value[2]["Ok"][1][0]["message"], "outer before");
    assert_eq!(value[2]["Ok"][1][1]["message"], "outer warning");
    assert_eq!(value[3]["Err"][0]["message"], "never failure");
    assert_eq!(value[4]["Err"][0]["message"], "integer division by zero");
    assert_eq!(value[5], 42);
    assert!(session.diagnostics().unwrap().is_empty());
    assert_eq!(session.call(&[]).unwrap(), value);
    let source = include_str!("../tests/fixtures/capture.telora");
    let bytes = compile_export(source, "uncaught").unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    session.initialize().unwrap();
    assert!(
        session
            .call(&[])
            .unwrap_err()
            .contains("uncaught afterwards")
    );
    assert_eq!(session.diagnostics().unwrap().len(), 1);
    let bytes = compile_export(source, "exhausted").unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    session.initialize().unwrap();
    session.store.set_fuel(200_000).unwrap();
    assert!(session.call(&[]).unwrap_err().contains("fuel"));
    assert_eq!(
        session.diagnostics().unwrap()[0].message,
        "entered exhausted scope"
    );
}

#[test]
fn array_callbacks_execute_in_wasm_with_closed_element_types() {
    let bytes = compile(include_str!("../tests/fixtures/array-ops.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([[11,12,13,14],[13,14],27,2,null,3,3,true,false,true,
        [1,2,3,4,5], [[0,"a"],[1,"b"]], [[1,"a"],[2,"b"]], null, [1,2,3], [1,11,2,12], {"Break":"done"}, {"Continue":42}])
    );
    let diagnostics = session.diagnostics().unwrap();
    assert_eq!(diagnostics.len(), 2);
    assert!(
        diagnostics
            .iter()
            .all(|d| d.warning && d.message == "flat_map once")
    );
}

fn graph(source: &str) -> Mir {
    let inventory = ["@src/main", "@src/input.json"]
        .into_iter()
        .chain(static_sources::BUILTINS.iter().map(|(name, _)| *name))
        .map(|name| ModuleSpec {
            name: name.into(),
            kind: if name == "@src/input.json" {
                telora_core::mir::ModuleKind::Data
            } else {
                telora_core::mir::ModuleKind::Source
            },
            native: static_sources::native_module(name),
            implicit_imports: if name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        Ok(if name == "@src/main" {
            source
        } else {
            static_sources::BUILTINS
                .iter()
                .find(|(module, _)| *module == name)
                .unwrap()
                .1
        }
        .into())
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{}", mir.dump());
    mir
}

#[test]
fn enums_patterns_and_propagation_use_full_prelude() {
    let bytes = compile(include_str!("../tests/fixtures/enums.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([42, null, "Idle", {"Number":42}])
    );
}

#[test]
fn properties_reduce_and_query_inside_wasm() {
    let bytes = compile(include_str!("../tests/fixtures/properties.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.eval().unwrap(), serde_json::json!([42, "amount"]));
}

#[test]
fn construction_checks_run_sealed_generic_checkers() {
    let bytes = compile(include_str!(
        "../../telora-native/tests/fixtures/construction-checks.telora"
    ))
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!(42));
    let diagnostics = session.diagnostics().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].warning);
    assert_eq!(diagnostics[0].message, "checker initialized");
}

#[test]
fn checks_and_macros_record_one_failure_with_rule_and_subject_origins() {
    let source = include_str!("../tests/fixtures/check-diagnostics.telora");
    for (name, message) in [
        ("rejected", "positive required"),
        ("raised", "raised message"),
        ("failed", "failed message"),
        ("variant", "positive variant"),
        ("unwrapped", "unwrap message"),
    ] {
        let bytes = compile_export(source, name).unwrap();
        let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
        session.initialize().unwrap();
        assert!(session.call(&[]).unwrap_err().contains(message));
        let ds = session.diagnostics().unwrap();
        assert_eq!(ds.len(), 1);
        assert!(!ds[0].warning);
        assert_eq!(ds[0].message, message);
        assert_eq!(ds[0].subjects.len(), 1);
        assert_ne!(ds[0].origin, ds[0].subjects[0]);
        assert_eq!(
            &source[ds[0].subjects[0][1] as usize..ds[0].subjects[0][2] as usize],
            "-7"
        );
        assert!(session.call(&[]).is_err());
        assert_eq!(session.diagnostics().unwrap().len(), 1);
    }
    let bytes = compile_export(source, "warned").unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!(42));
    let ds = session.diagnostics().unwrap();
    assert_eq!(
        ds.iter().map(|d| d.message.as_str()).collect::<Vec<_>>(),
        ["string warning", "blame warning", "result warning"]
    );
    assert!(ds.iter().all(|d| d.warning));
    assert!(ds[0].subjects.is_empty());
    assert_eq!(ds[1].subjects.len(), 1);
    assert_eq!(ds[2].subjects.len(), 1);
}

#[test]
fn semantic_value_contract_survives_artifact_reload() {
    let source = include_str!("../tests/fixtures/semantic-value.telora");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    assert_eq!(
        session.manifest.value_type,
        Some(session.manifest.entry_type)
    );
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!({"number": 42, "nested": [null, true, "hello"]})
    );
    let bytes = compile_export(source, "identity").unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    let input = serde_json::json!({"z": [1, 2.5, false, null], "a": "nested input"});
    assert_eq!(session.call(&[input.clone()]).unwrap(), input);
}

#[test]
fn data_injection_precedes_property_initialization_and_is_single_use() {
    let mir = graph(include_str!("../tests/fixtures/entry.telora"));
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let bytes = crate::compile_executable(&mir.seal_export(export).unwrap()).unwrap();
    let mut missing = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    assert!(
        missing
            .initialize()
            .unwrap_err()
            .contains("not been injected")
    );
    let mut sources = mir.sources;
    let source = sources.add("input.json", "{\"number\":42}");
    let plan = telora_core::data_plan::parse_registered(
        &sources,
        source,
        telora_core::data_plan::Format::Json,
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    let symbol = session.manifest.data_modules[0].symbol;
    assert!(missing.inject_data(symbol, &plan).is_err());
    let mut conflicting = telora_core::SourceDatabase::default();
    conflicting.add("different source using the same id", "");
    assert!(session.register_data_sources(&conflicting, &plan).is_err());
    session.register_data_sources(&sources, &plan).unwrap();
    session.inject_data(symbol, &plan).unwrap();
    assert!(session.inject_data(symbol, &plan).is_err());
    session.initialize().unwrap();
    let lookup = session
        .instance
        .get_typed_func::<i32, i32>(&session.store, "telora_source_name")
        .unwrap();
    for (id, expected) in [
        (source.get(), "input.json"),
        (u32::MAX, "source:4294967295"),
    ] {
        let span = lookup.call(&mut session.store, id as i32).unwrap() as usize;
        let memory = session.memory.data(&session.store);
        let pointer = u32::from_le_bytes(memory[span..span + 4].try_into().unwrap()) as usize;
        let length = u32::from_le_bytes(memory[span + 4..span + 8].try_into().unwrap()) as usize;
        assert_eq!(&memory[pointer..pointer + length], expected.as_bytes());
    }
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([{"number":42},42])
    );
    assert!(session.inject_data(symbol, &plan).is_err());
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
fn failed_demands_keep_failure_identity_instead_of_running_state() {
    let mir = graph("def broken: Int = 1 / 0; export def answer = broken;");
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let executable = mir.seal_export(export).unwrap();
    let plan = crate::plan::Plan::new(&executable).unwrap();
    let bytes = crate::compile_executable(&executable).unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    assert!(session.initialize().is_err());
    let memory = session.memory.data(&session.store);
    let mut failed = 0;
    for &offset in plan.demands.values() {
        let offset = offset as usize;
        let state = u32::from_le_bytes(memory[offset..offset + 4].try_into().unwrap());
        assert_ne!(state, 1, "unwound demand remained Running");
        if state == 3 {
            failed += 1;
            let error = u32::from_le_bytes(memory[offset + 4..offset + 8].try_into().unwrap());
            assert_ne!(error, 0, "Failed demand lost its diagnostic identity");
        }
    }
    assert!(failed > 0);
    assert_eq!(session.diagnostics().unwrap().len(), 1);
    assert!(session.initialize().is_err());
    assert_eq!(session.diagnostics().unwrap().len(), 1);
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
    // Cross a memory.grow boundary before invoking existing closures. Rust
    // stack/static state, table slots and the new heap allocation must not alias.
    let initial_size = session.memory.data(&session.store).len();
    let marker = vec![0xa5; initial_size + 65536];
    let allocation = session.allocate(marker.len()).unwrap() as usize;
    session.write(allocation, &marker).unwrap();
    assert!(session.memory.data(&session.store).len() > initial_size);
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
    assert_eq!(&after[allocation..allocation + marker.len()], &marker);
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
