use telora_core::{
    mir::Mir,
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};

mod content;
mod data;
mod debug;
mod dynamic;
mod equality;
mod expression_locals;
mod format;
mod host_memory;
mod interpreters;
mod json;
mod records;
mod reflection;
mod regex;
mod service_sources;
mod services;
mod source_ranges;
mod tail_calls;
mod test_descriptions;
mod transform_service;
mod variant_origins;

#[test]
fn engine_stops_unbounded_loops_and_allocation() {
    for (source, fuel, reason) in [
        (
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/telora-wasm/tests/fixtures/unbounded-loop.telora"
            ))
            .expect("read test source"),
            1_000_000,
            "fuel",
        ),
        (
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../crates/telora-wasm/tests/fixtures/unbounded-allocation.telora"
            ))
            .expect("read test source"),
            100_000_000,
            "growth",
        ),
    ] {
        let bytes = compile(source).unwrap();
        let mut session = crate::session::Session::load(&bytes, fuel).unwrap();
        session.initialize().unwrap();
        if reason == "growth" {
            // Exercise the same engine limiter with a smaller bound so the test
            // does not need to allocate the production limit of 1 GiB.
            let bound = session.memory.data(&session.store).len() + 1024 * 1024;
            *session.store.data_mut() = wasmi::StoreLimitsBuilder::new()
                .memory_size(bound)
                .trap_on_grow_failure(true)
                .build();
        }
        let error = session.call(&[]).unwrap_err();
        assert!(error.contains(reason), "{error}");
        assert!(session.diagnostics().unwrap().is_empty());
    }
}

#[test]
fn source_indexes_preserve_original_eol_byte_ranges() {
    for eol in ["\n", "\r\n", "\r"] {
        let text = format!("中文🙂x{eol}next{eol}");
        let mut database = telora_core::SourceDatabase::default();
        let id = database.add("test", &text);
        let file = database.get(id);
        let source = crate::artifact::Source::from_file(file);
        assert!(
            serde_json::to_value(&source)
                .unwrap()
                .get("lines")
                .is_none(),
            "BOLs belong only to Guest storage, not the serialized manifest"
        );
        let loc =
            telora_core::Loc::from_usize(id, "中文".len()..text.find("next").unwrap() + 4).unwrap();
        let packed = file.coordinates(loc);
        assert_eq!(source.position(packed.start()), (1, 7));
        assert_eq!(source.position(packed.end()), (2, 5));
        assert_eq!(file.byte_location(packed), Some(loc));
        assert_eq!(
            source.lines,
            vec![
                [0, "中文🙂x".len() as u32],
                [
                    text.find("next").unwrap() as u32,
                    (text.len() - eol.len()) as u32
                ],
                [text.len() as u32, text.len() as u32],
            ]
        );
    }
}

#[test]
fn diagnostics_are_equivalent_across_line_endings() {
    let source = "# comment\nexport def answer: Fn() -> Never = fn() {\n    let value = dbg!((\n        42\n    ));\n    fail!(\"same failure\", value)\n};\n";
    let mut expected = None;
    for eol in ["\n", "\r\n", "\r"] {
        let text = source.replace('\n', eol);
        let bytes = compile(&text).unwrap();
        assert_eq!(
            bytes,
            compile(&text).unwrap(),
            "same input must remain deterministic"
        );
        let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
        session.initialize().unwrap();
        assert!(session.call(&[]).is_err());
        let diagnostic = session.diagnostics().unwrap().remove(0);
        let rendered = diagnostic.render(&session.manifest);
        if let Some(expected) = &expected {
            assert_eq!(&rendered, expected);
        } else {
            expected = Some(rendered);
        }
    }
}

#[test]
fn multiline_string_values_ignore_source_eol() {
    let source = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/language/src/test/string-eol/values.telora"
    ))
    .unwrap()
    .replace("\r\n", "\n");
    for name in [
        "physical_newlines",
        "explicit_carriage_returns",
        "continuations",
    ] {
        for eol in ["\n", "\r\n", "\r"] {
            let bytes = compile_export(&source.replace('\n', eol), name).unwrap();
            let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
            session.initialize().unwrap();
            assert_eq!(session.call(&[]).unwrap(), serde_json::json!(true));
        }
    }
}

#[test]
fn runtime_diagnostics_preserve_high_line_bits() {
    let source = format!(
        "{}export def answer: Fn() -> Never = fn() {{ fail!(\"high line\", 42) }};",
        "\n".repeat(70_000)
    );
    let bytes = compile(&source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert!(session.call(&[]).is_err());
    let diagnostics = session.diagnostics().unwrap();
    let loc = telora_core::source::SourceCoordinates(diagnostics[0].origin);
    assert_eq!(loc.start() >> 32, 70_000);
    let name = &session
        .manifest
        .sources
        .iter()
        .find(|file| file.id == loc.source())
        .unwrap()
        .name;
    assert!(
        diagnostics[0]
            .render(&session.manifest)
            .starts_with(&format!("{name}:70001:"))
    );
}

#[test]
fn runtime_gap_regressions_keep_closed_calls_and_language_failures() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/runtime-gaps.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([
            42,
            [1.5, -1.5, 1.5, -1.5, 1.0e300_f64 % 3.0],
            "Function has no JSON codec",
            "cannot encode Type",
            "NonFiniteFloat"
        ])
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn sequence_contributions_use_sealed_layouts_and_preserve_evaluation_order() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/sequences.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([
            [1,"text",2], null, [[1,2],3], [1,"text",3],
            [1], [1], [1,2], [[],[1]], [{"value":42},{"value":42},{"value":42}],
            [{"value":42}], [1,2,3], [1,{"value":2},"hi"], 42, 2
        ])
    );
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/sequence-origins.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result[0], serde_json::json!([7, 7]));
    assert_eq!(result[1], serde_json::json!([7, 8, 7]));
    assert_eq!(result[4], "tuple spread failed");
    assert_eq!(result[5], "array spread failed");
    assert_eq!(result[6], "uninhabited tuple");
    let diagnostics = session.diagnostics().unwrap();
    assert_eq!(
        diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>(),
        ["first", "middle", "last", "a", "b", "c"]
    );
    assert!(diagnostics.iter().all(|d| d.warning));
    assert_eq!(
        diagnostic_point(&result[2]["labels"][1]["location"]["start"]),
        point(source, source.find("42").unwrap())
    );
    assert_eq!(
        diagnostic_point(&result[3]["labels"][1]["location"]["start"]),
        point(source, source.find("(...original, 3)").unwrap())
    );
}

#[test]
fn string_operations_preserve_unicode_and_line_semantics() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/string-ops.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([
            3,
            true,
            true,
            true,
            true,
            false,
            "a:é:🦀",
            "",
            "a\n",
            ["a", "é", ""],
            ["", "é", "🦀", ""],
            ["a", "b", ""],
            [""],
            "ba",
            ":é:é:",
            "  a\n\n\r\n  b",
            "\n",
            "x\n",
            "a\r\nb\n  plain",
            "String indentation width must be non-negative",
            "String margin marker must not be empty"
        ])
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn dictionary_operations_use_sorted_columns_and_closed_callbacks() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/dict-ops.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 5_000_000).unwrap();
    session.initialize().unwrap();
    let value = session.call(&[]).unwrap();
    assert_eq!(
        value,
        serde_json::json!([
            ["a","m","z","é"], [1,2,3,4], [["a",1],["m",2],["z",3],["é",4]],
            {"a":11,"m":12,"z":13,"é":14}, {"z":13,"é":14}, 1234,
            1, null, {"a":10,"b":20,"m":2,"z":3,"é":4},
            {"a":1,"m":2,"z":3,"é":4}, {"a":1,"m":2,"z":3,"é":4}, [], 42,
            "std/dict.from_pairs contains duplicate field \"same\"", [[1,2],[2,3],[3,4],[4,5]], 4,
            [["z",3],["a",1],["é",4],["m",2]], [2,3]
        ])
    );
    assert_eq!(session.diagnostics().unwrap().len(), 4);
    assert!(
        session
            .diagnostics()
            .unwrap()
            .iter()
            .all(|d| d.warning && d.message == "visited")
    );
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/dict-ops.telora"
        ))
        .expect("read test source"),
        "sort_input",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let pairs = (0..200)
        .rev()
        .map(|index| serde_json::json!([format!("key-{index:04}"), index]))
        .collect::<Vec<_>>();
    let value = session.call(&[serde_json::json!(pairs)]).unwrap();
    assert_eq!(
        value[0],
        serde_json::json!(
            (0..200)
                .map(|index| format!("key-{index:04}"))
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(value[1], serde_json::json!((0..200).collect::<Vec<_>>()));
    assert_eq!(value[2], serde_json::json!(pairs));
}

#[test]
fn diagnostic_scopes_capture_reports_and_resume_outer_execution() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/capture.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
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
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/capture.telora"
    ))
    .expect("read test source");
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
    assert!(
        session.diagnostics().unwrap()[0].initialization.is_none(),
        "initialization scope must end before entry execution"
    );
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
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/array-ops.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
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

#[test]
fn array_inner_buffer_preserves_old_versions_and_splits_branches() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/array-inner-buffer.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([[1], [1, 2], [1, 2, 3], [1, 9], [1, 2, 3, 4]])
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
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/enums.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([42, null, "Idle", {"Number":42}])
    );
}

#[test]
fn properties_reduce_and_query_inside_wasm() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/properties.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([42, "amount", 42, 42, true])
    );
}

#[test]
fn construction_checks_run_sealed_generic_checkers() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/runtime/construction-checks.telora"
        ))
        .expect("read test source"),
    )
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
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/check-diagnostics.telora"
    ))
    .expect("read test source");
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
        assert_eq!(source_slice(source, ds[0].subjects[0]), "-7");
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
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/semantic-value.telora"
    ))
    .expect("read test source");
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
    let mir = graph(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/entry.telora"
        ))
        .expect("read test source"),
    );
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
    let source = sources
        .try_add_data("input.json", "{\"number\":42}".into())
        .unwrap();
    let format = telora_core::data_plan::Format::Json;
    let mut session = crate::session::Session::load(&bytes, 2_000_000).unwrap();
    let symbol = session.manifest.data_modules[0].symbol;
    let value = missing
        .parse_data_source(sources.get(source), format)
        .unwrap()
        .unwrap();
    assert!(missing.inject_data_value(symbol, value).is_err());
    let mut conflicting = telora_core::SourceDatabase::default();
    let conflict = conflicting.add("different source using the same id", "");
    assert!(
        session
            .parse_data_source(conflicting.get(conflict), format)
            .is_err()
    );
    let value = session
        .parse_data_source(sources.get(source), format)
        .unwrap()
        .unwrap();
    session.inject_data_value(symbol, value).unwrap();
    assert!(session.inject_data_value(symbol, value).is_err());
    session.initialize().unwrap();
    let lookup = session
        .instance
        .get_typed_func::<i32, i32>(&session.store, "telora_source_name")
        .unwrap();
    for (id, expected) in [(source.get(), "input.json")] {
        let span = lookup.call(&mut session.store, id as i32).unwrap() as usize;
        let output = session.output();
        let pointer = u64::from(output.word(span as u64).unwrap());
        let length = u64::from(output.word(span as u64 + 4).unwrap());
        assert_eq!(output.bytes(pointer, length).unwrap(), expected.as_bytes());
    }
    assert_eq!(
        session.eval().unwrap(),
        serde_json::json!([{"number":42},42])
    );
    assert!(session.inject_data_value(symbol, value).is_err());
    assert!(lookup.call(&mut session.store, -1).is_err());
}

#[test]
fn function_dependency_roots_use_planned_demand_indices() {
    let mir = graph(
        r#"
        def base: String = "kept";
        export def answer: Fn() -> String = fn() { base };
    "#,
    );
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let executable = mir.seal_export(export).unwrap();
    let plan = crate::plan::Plan::new(&executable).unwrap();
    let base = plan
        .globals
        .keys()
        .copied()
        .find(|symbol| mir.symbols[symbol.index()].name == "base")
        .unwrap();
    let key = plan.globals[&base];
    let offset = plan.demands[&key];
    let expected = (offset - crate::compose::static_base().unwrap()) / crate::abi::DEMAND_BYTES;
    assert!(
        plan.function_demand_roots
            .iter()
            .any(|roots| roots == &[expected])
    );
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
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/arithmetic-errors.telora"
    ))
    .expect("read test source");
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
    let mir = graph("def broken: Int = 1 / 0; export def answer: Int = broken;");
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
        compile("def loop: Fn(Int) -> Int = fn(n: Int) -> Int { loop(n + 1) }; export def answer: Int = loop(0);")
            .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000).unwrap();
    // Startup work may change; this test constrains the initialization loop.
    session.store.set_fuel(1000).unwrap();
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
    let bytes = compile("export def answer: Int = 42;").unwrap();
    assert_eq!(bytes, compile("export def answer: Int = 42;").unwrap());
    assert!(
        bytes
            .windows(b"|owner=answer|role=demand|hir=".len())
            .any(|window| window == b"|owner=answer|role=demand|hir=")
    );
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
    let pointer = instance
        .get_typed_func::<u32, u32>(&store, "telora_heap_address")
        .unwrap()
        .call(&mut store, pointer as u32)
        .unwrap() as usize;
    let memory = instance.get_memory(&store, "memory").unwrap();
    let bytes = memory.data(&store);
    assert_eq!(
        i64::from_le_bytes(
            bytes[pointer + crate::abi::DATA as usize..pointer + crate::abi::DATA as usize + 8]
                .try_into()
                .unwrap()
        ),
        42
    );
}

#[test]
fn aggregates_use_closed_layouts_and_classified_heap_tables() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/aggregates.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
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
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/dictionaries.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
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
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/call-input.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
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
    assert!(
        session
            .output()
            .bytes(allocation as u64, marker.len() as u64)
            .unwrap()
            == marker,
        "logical allocation must survive arena relocation"
    );
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
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/functions.telora"
        ))
        .expect("read test source"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/local-recursion.telora"
        ))
        .expect("read test source"),
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/short-circuit.telora"
        ))
        .expect("read test source"),
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
        let pointer = instance
            .get_typed_func::<u32, u32>(&store, "telora_heap_address")
            .unwrap()
            .call(&mut store, pointer as u32)
            .unwrap() as usize;
        let memory = instance.get_memory(&store, "memory").unwrap();
        let bytes = memory.data(&store);
        assert_eq!(
            i64::from_le_bytes(
                bytes[pointer + crate::abi::DATA as usize..pointer + crate::abi::DATA as usize + 8]
                    .try_into()
                    .unwrap()
            ),
            42,
            "{source}"
        );
    }
}
// Expected coordinates for LF-only test assets, independent of the ABI packer.
fn point(source: &str, byte: usize) -> u64 {
    let prefix = &source[..byte];
    let line = prefix.bytes().filter(|&byte| byte == b'\n').count();
    let column = prefix.rsplit('\n').next().unwrap().len();
    ((line as u64) << 32) | column as u64
}

fn source_slice(source: &str, words: [u32; 5]) -> &str {
    let index = telora_core::source::LineIndex::new(source).unwrap();
    let loc = telora_core::source::SourceCoordinates(words);
    &source[index.byte(loc.start()).unwrap() as usize..index.byte(loc.end()).unwrap() as usize]
}

fn diagnostic_point(value: &serde_json::Value) -> u64 {
    (value["line"].as_u64().unwrap() << 32) | value["offset"].as_u64().unwrap()
}
