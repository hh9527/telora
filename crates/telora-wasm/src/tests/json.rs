use super::*;

#[test]
fn data_parse_rejection_preserves_original_input_location() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/data-parse-rejections.telora"
    ))
    .expect("read test source");
    for (export, input) in [
        ("toml_rejected", "\"duplicate=1"),
        ("yaml_rejected", "\"duplicate: 1"),
    ] {
        let bytes = compile_export(source, export).unwrap();
        let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
        session.initialize().unwrap();
        let result = session.call(&[]).unwrap();
        assert!(result["message"].as_str().unwrap().contains("duplicate"));
        assert_eq!(
            result["labels"][1]["location"]["start"],
            source.find(input).unwrap()
        );
        assert!(session.diagnostics().unwrap().is_empty());
    }
}
#[test]
fn codec_enum_rename_collision_is_reported_once() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/codec-enum-collision.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap()["message"],
        "duplicate external variant name"
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_rename_collision_is_a_captured_evaluation_error() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/codec-rename-errors.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result[0]["message"], "duplicate external field name");
    assert_eq!(result[1], serde_json::json!({"a_b":1,"aB":2}));
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_parse_display_markers_must_be_paired() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/codec-bridge-errors.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    for index in 0..2 {
        assert_eq!(
            result[index]["message"],
            "std/string.decode_by_parse and std/string.encode_by_display must be used together"
        );
    }
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_untagged_rejects_ambiguous_and_incompatible_declarations() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/codec-untagged-errors.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(
        result[0]["message"],
        "untagged Enum may contain at most one unit variant"
    );
    assert_eq!(
        result[1]["message"],
        "rename_all is not meaningful on an untagged Enum"
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_decode_enum_checks_run_once_per_candidate() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/codec-decode-enum-effects.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!([true, true]));
    let diagnostics = session.diagnostics().unwrap();
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0].message, "tag visited");
    assert_eq!(diagnostics[1].message, "untagged visited");
}

#[test]
fn codec_decode_untagged_keeps_rejection_evidence_and_propagates_failure() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/codec-decode-enum-errors.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    for reports in result.as_array().unwrap() {
        assert_eq!(reports.as_array().unwrap().len(), 1);
    }
    assert_eq!(
        result[0][0]["message"],
        "$: value matches no untagged Enum variant ($.number: expected Int; $: expected String)"
    );
    assert_eq!(
        result[0][0]["labels"][1]["location"]["start"],
        source.find("Value.String(\"bad\")").unwrap()
    );
    assert_eq!(
        result[1][0]["message"],
        "$: value ambiguously matches multiple untagged Enum variants"
    );
    assert_eq!(result[2][0]["message"], "candidate failed");
    assert_eq!(result[3][0]["message"], "duplicate external member name");
    assert_eq!(
        result[4][0]["message"],
        "rename_all is not meaningful on an untagged Enum"
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_decode_check_blames_are_reported_only_when_raised() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/codec-decode-check-origins.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    for (index, (message, needle)) in [
        ("positive count", "-7"),
        ("positive record", "-8"),
        ("positive parsed", "\"-9\""),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(result[index].as_array().unwrap().len(), 1);
        assert_eq!(result[index][0]["message"], message);
        assert_eq!(
            result[index][0]["labels"][1]["location"]["start"],
            source.find(needle).unwrap()
        );
    }
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_decode_tuple_honors_array_slice_start() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/language/src/test/runtime-codec/codec-decode-tuples.telora"
        ))
        .expect("read test source"),
        "slice",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let function = session.entry().unwrap();
    let signature = session.manifest.types[session.manifest.entry_type as usize]
        .arguments
        .clone();
    let value = session
        .input(signature[0], &serde_json::json!([false, 7, "x", false]), 0)
        .unwrap();
    // Array slicing has no source syntax; exercise a valid ABI slice descriptor.
    session
        .write(value as usize + 20, &1u32.to_le_bytes())
        .unwrap();
    session
        .write(value as usize + 24, &3u32.to_le_bytes())
        .unwrap();
    let args = session.allocate(4).unwrap();
    session.write(args as usize, &value.to_le_bytes()).unwrap();
    let invoke = session
        .instance
        .get_typed_func::<(i32, i32), i32>(&session.store, "telora_invoke")
        .unwrap();
    let result = invoke
        .call(&mut session.store, (function as i32, args as i32))
        .unwrap();
    let output = crate::output::Output {
        memory: session.memory.data(&session.store),
        manifest: &session.manifest,
    };
    assert_eq!(
        output.json(result as u64, signature[1], 0).unwrap(),
        serde_json::json!(true)
    );
}

#[test]
fn codec_decode_nested_error_keeps_path_and_leaf_origin() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/codec-decode-nested-origin.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result["message"], "$[0][1]: expected Int");
    assert_eq!(
        result["labels"][1]["location"]["start"],
        source.find("Value.String(").unwrap()
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_decode_mismatch_retains_input_origin() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/codec-decode-origins.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result["message"], "$: expected Int");
    assert_eq!(
        result["labels"][1]["location"]["start"],
        source.find("Value.String(").unwrap()
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_display_failure_propagates_without_duplicate_diagnostics() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/codec-display-errors.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result[0].as_array().unwrap().len(), 1);
    assert_eq!(result[1].as_array().unwrap().len(), 1);
    assert_eq!(
        result[0][0]["message"],
        "text codec requires a DisplayBy property"
    );
    assert_eq!(result[1][0]["message"], "display execution failed");
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn dictionary_index_uses_sorted_lookup_and_reports_missing_keys() {
    let bytes = compile(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/dict-index.telora"
        ))
        .expect("read test source"),
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result[0], 1);
    assert_eq!(result[1], 9);
    assert!(result[2]["message"].as_str().unwrap().contains("key"));
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_record_encoding_preserves_field_origins_and_empty_records() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/codec-record-origins.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result[0], serde_json::json!({}));
    assert_eq!(
        result[1]["labels"][1]["location"]["start"],
        source.find("12345").unwrap()
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn recursive_codec_record_encoding_executes_the_function_graph() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/codec-recursive-plan.telora"
        ))
        .expect("read test source"),
        "sample",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!({"next":{"next":null,"value":2},"value":1})
    );
}

#[test]
fn recursive_codec_planning_closes_a_finite_function_graph() {
    let mir = graph(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/telora-wasm/tests/fixtures/codec-recursive-plan.telora"
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
    let executable = mir.seal_export(export).unwrap();
    let plan = crate::plan::Plan::new(&executable).unwrap();
    let encoders: Vec<_> = plan
        .functions
        .keys()
        .filter_map(|key| match key.special {
            crate::plan::Special::Encode(source, _) => Some(source),
            _ => None,
        })
        .collect();
    // Link -> Option(Link) -> Link is a cycle, not an expansion tree.
    assert_eq!(encoders.len(), 3);
    assert!(
        encoders
            .iter()
            .any(|ty| mir.types[ty.index()].constructor == telora_core::mir::TypeConstructor::Int)
    );
    assert!(
        encoders.iter().any(
            |ty| mir.types[ty.index()].constructor == telora_core::mir::TypeConstructor::Option
        )
    );
}

#[test]
fn json_parse_error_blames_the_original_text() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/json-parse-origins.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(
        result["labels"][1]["location"]["start"],
        source.find("\"[1,]\"").unwrap()
    );
    assert_eq!(
        result["message"],
        r#"<json string>: invalid syntax, expected one of: '"', 'false', '{', '[', 'null', <number>, 'true'"#
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn json_parse_materializes_postorder_plan_with_closed_types() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/language/src/test/runtime-data/json-parse.telora"
        ))
        .expect("read test source"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        assert_eq!(
            session.call(&[]).unwrap(),
            serde_json::json!([
                "{\"a\":[1,1,true,null,\"中\",{\"k\":false}],\"z\":[]}",
                true,
                true,
                true,
                true,
                true
            ])
        );
    }
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn stringify_rejections_are_captured_once_and_preserve_subjects() {
    let source = &std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/telora-wasm/tests/fixtures/json-errors.telora"
    ))
    .expect("read test source");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    for (index, message) in [
        "JSON cannot encode Bytes",
        "JSON cannot encode temporal values; use a codec first",
        "std/json.stringify_pretty indent must be between 0 and 16",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(result[index]["message"], *message);
    }
    assert_eq!(
        result[0]["labels"][1]["location"]["start"],
        source.find("Value.Bytes(b").unwrap()
    );
    assert_eq!(result[3], "true");
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn stringify_traverses_closed_value_layouts() {
    let bytes = compile_export(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/language/src/test/runtime-data/json-stringify.telora"
        ))
        .expect("read test source"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        let result = session.call(&[]).unwrap();
        assert_eq!(
            result[0],
            "{\"a\":[null,true,false,-7,1,\"中\\n\\\"\"],\"z\":{}}"
        );
        let expected: serde_json::Value =
            serde_json::from_str(result[0].as_str().unwrap()).unwrap();
        assert_eq!(result[1], serde_json::to_string_pretty(&expected).unwrap());
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(result[2].as_str().unwrap()).unwrap(),
            expected
        );
        assert_eq!(result[3], "[]");
    }
}
