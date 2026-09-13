use super::*;

#[test]
fn codec_display_failure_propagates_without_duplicate_diagnostics() {
    let bytes = compile(include_str!(
        "../../tests/fixtures/codec-display-errors.telora"
    ))
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
fn codec_display_bridge_invokes_the_sealed_formatter() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/codec-display.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), "localhost:8080");
}

#[test]
fn codec_parse_display_markers_must_be_paired() {
    let bytes = compile(include_str!(
        "../../tests/fixtures/codec-bridge-errors.telora"
    ))
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
    let bytes = compile(include_str!(
        "../../tests/fixtures/codec-untagged-errors.telora"
    ))
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
fn codec_untagged_uses_payload_or_null() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/codec-untagged.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([null, 7, "x"])
    );
}

#[test]
fn codec_enum_rename_collision_is_reported_once() {
    let bytes = compile(include_str!(
        "../../tests/fixtures/codec-enum-collision.telora"
    ))
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
fn codec_enum_rename_keeps_sealed_variant_indices() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/codec-enum-rename.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(["noValue",{"hasValue":42}])
    );
}

#[test]
fn codec_rename_collision_is_a_captured_evaluation_error() {
    let bytes = compile(include_str!(
        "../../tests/fixtures/codec-rename-errors.telora"
    ))
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(result[0]["message"], "duplicate external field name");
    assert_eq!(result[1], serde_json::json!({"a_b":1,"aB":2}));
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn codec_record_rename_uses_demanded_property_and_sorted_external_keys() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/codec-rename.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!({"aValue":"text","zValue":7})
    );
}

#[test]
fn codec_enums_encode_closed_names_and_recursive_payloads() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/codec-enum.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(["Empty",{"Child":{"Number":42}},"Empty",{"Ok":7},{"Err":"bad"}])
    );
}

#[test]
fn codec_newtypes_encode_their_closed_payloads() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/codec-newtype.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!([1, 2]));
}

#[test]
fn dictionary_index_uses_sorted_lookup_and_reports_missing_keys() {
    let bytes = compile(include_str!("../../tests/fixtures/dict-index.telora")).unwrap();
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
    let source = include_str!("../../tests/fixtures/codec-record-origins.telora");
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
        include_str!("../../tests/fixtures/codec-recursive-plan.telora"),
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
    let mir = graph(include_str!(
        "../../tests/fixtures/codec-recursive-plan.telora"
    ));
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
fn codec_scalar_encoding_uses_closed_payload_identities() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/codec-scalars.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 18])
    );
}

#[test]
fn json_parse_error_blames_the_original_text() {
    let source = include_str!("../../tests/fixtures/json-parse-origins.telora");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    assert_eq!(
        result["labels"][1]["location"]["start"],
        source.find("\"[1,]\"").unwrap()
    );
    assert_eq!(result["message"], "expected value at line 1 column 4");
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn json_parse_materializes_postorder_plan_with_closed_types() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/json-parse.telora"),
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
    let source = include_str!("../../tests/fixtures/json-errors.telora");
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
        include_str!("../../tests/fixtures/json-stringify.telora"),
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
