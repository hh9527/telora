use super::*;

#[test]
fn dyn_fields_observe_records_and_sorted_dictionaries() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/dynamic-fields.telora"),
        "checks",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 10])
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn dyn_sequences_retain_closed_element_types() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/dynamic-sequences.telora"),
        "checks",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(session.call(&[]).unwrap(), serde_json::json!(vec![true; 8]));
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn dyn_kind_classifies_closed_values() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/dynamic-kind.telora"),
        "checks",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 16])
    );
}

#[test]
fn dyn_variants_use_sealed_payload_layouts() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/dynamic-variants.telora"),
        "checks",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 13])
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn late_array_field_evidence_survives_an_empty_match_arm() {
    let bytes = compile(include_str!("../../tests/fixtures/late-array-field.telora")).unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session
            .call(&[serde_json::json!({"p":{"a":["value"]}}), true.into()])
            .unwrap(),
        serde_json::json!(["value"])
    );
    assert_eq!(
        session
            .call(&[serde_json::json!({"p":{"a":["value"]}}), false.into()])
            .unwrap(),
        serde_json::json!([])
    );
}

#[test]
fn display_properties_initialize_and_render_nested_values_inside_wasm() {
    let bytes = compile_export(
        include_str!("../../tests/fixtures/display-by.telora"),
        "inspect",
    )
    .unwrap();
    let mut session = crate::session::Session::load(&bytes, 20_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!([
            "localhost:8080",
            "localhost:8080",
            "api@localhost:8080 {ready} -0 api",
            "endpoint=explicit(localhost:8080)",
            ["host", "port"],
            "absent"
        ])
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn reflection_reads_linked_closed_type_descriptors() {
    let source = include_str!("../../tests/fixtures/reflection.telora");
    let bytes = compile_export(source, "checks").unwrap();
    assert_eq!(bytes, compile_export(source, "checks").unwrap());
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 30])
    );
    assert_eq!(
        session.call(&[]).unwrap(),
        serde_json::json!(vec![true; 30])
    );
    assert!(session.diagnostics().unwrap().is_empty());
}

#[test]
fn reflection_errors_are_captured_and_dyn_fields_keep_their_origins() {
    let source = include_str!("../../tests/fixtures/reflection-effects.telora");
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 10_000_000).unwrap();
    session.initialize().unwrap();
    let result = session.call(&[]).unwrap();
    for (index, expected) in [
        "std/type-desc.fields expects Struct",
        "std/type-desc.variants expects Enum",
        "Dyn field access expects Struct",
        "Dyn member index must be a non-negative u32",
        "Dyn member index must be a non-negative u32",
        "field index 1 is out of range",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(result[index], *expected);
    }
    assert_eq!(
        result[6]["labels"][1]["location"]["start"],
        source.find("42").unwrap()
    );
    for (index, expected) in [
        "Dyn variant index is 1, not 0",
        "Dyn variant access expects Enum",
        "Dyn member index must be a non-negative u32",
        "Dyn member index must be a non-negative u32",
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(result[index + 7], *expected);
    }
    assert_eq!(
        result[11]["labels"][1]["location"]["start"],
        source.find("12345").unwrap()
    );
    assert_eq!(
        result[12]["labels"][1]["location"]["start"],
        source.find("23456").unwrap()
    );
    assert_eq!(
        result[13]["labels"][1]["location"]["start"],
        source.find("34567").unwrap()
    );
    for index in [14, 15] {
        assert_eq!(
            result[index]["labels"][1]["location"]["start"],
            source.find("42").unwrap()
        );
    }
    assert!(session.diagnostics().unwrap().is_empty());
}
