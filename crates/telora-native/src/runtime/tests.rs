use super::*;
use crate::test_support::{graph, value_type};

const SOURCE: &str = include_str!("../../tests/fixtures/runtime.telora");

#[test]
fn semantic_json_reads_native_graph_before_and_after_publication() {
    use telora_core::data_plan::{self, Format};
    let mut mir = crate::test_support::graph_with(
        "import \"std/value\" { Value }; export def answer = Value.None;",
        telora_core::static_sources::BUILTINS,
    );
    let source = mir.sources.add(
        "output.json",
        r#"{"z":[null,true,false,-42,1.25],"a":"line\n\"quoted\""}"#,
    );
    let plan = data_plan::parse_registered(&mir.sources, source, Format::Json).unwrap();
    let sealed = mir.seal().unwrap();
    let contract = DataContract::from_mir(&sealed).unwrap();
    let mut rt = Runtime::new(&sealed).unwrap();
    let data = rt.materialize_data(&contract, &plan).unwrap();
    let expected = r#"{"a":"line\n\"quoted\"","z":[null,true,false,-42,1.25]}"#;
    assert_eq!(rt.semantic_json(&contract, &data).unwrap(), expected);
    let roots = rt.publish(&[data]).unwrap();
    assert_eq!(rt.semantic_json(&contract, &roots[0]).unwrap(), expected);
}

#[test]
fn data_plans_materialize_json_yaml_toml_with_source_locations() {
    use telora_core::data_plan::{self, Format};
    for (format, text) in [
        (Format::Json, "{\"a\":42,\"items\":[true,false]}"),
        (Format::Yaml, "a: 42\nitems: [true, false]\n"),
        (Format::Toml, "a = 42\nitems = [true, false]\n"),
    ] {
        let mut mir = crate::test_support::graph_with(
            "import \"std/value\" { Value }; export def answer = Value.None;",
            telora_core::static_sources::BUILTINS,
        );
        let source = mir.sources.add("data-input", text);
        let plan = data_plan::parse_registered(&mir.sources, source, format).unwrap();
        let sealed = mir.seal().unwrap();
        let contract = DataContract::from_mir(&sealed).unwrap();
        let mut rt = Runtime::new(&sealed).unwrap();
        let data = rt.materialize_data(&contract, &plan).unwrap();
        let roots = rt.publish(&[data]).unwrap();
        let dict = rt.enum_payload(&roots[0]).unwrap().unwrap().to_owned();
        let (key, value) = rt.dict_entry(&dict, 0).unwrap();
        assert_eq!(rt.text(key).unwrap().as_str(), "a");
        let value = value.to_owned();
        let number = rt.enum_payload(&value).unwrap().unwrap();
        assert_eq!(rt.scalar_bits(number).unwrap(), 42);
        assert_eq!(number.location()[1] as usize, text.find("42").unwrap());
        assert_eq!(number.location()[0], key.location()[0]);
        let (_, items) = rt.dict_entry(&dict, 1).unwrap();
        let items = items.to_owned();
        let array = rt.enum_payload(&items).unwrap().unwrap().to_owned();
        assert_eq!(rt.array_len(&array).unwrap(), 2);
        let first = rt.array_get(&array, 0).unwrap().to_owned();
        assert_eq!(
            rt.variant_name(first.type_id(), rt.enum_tag(&first).unwrap())
                .unwrap(),
            "True"
        );
    }
}

#[test]
fn yaml_bytes_share_backing_with_slices_and_toml_keeps_date_tags() {
    use telora_core::data_plan::{self, Format};
    let mut mir = crate::test_support::graph_with(
        "import \"std/value\" { Value }; export def answer = Value.None;",
        telora_core::static_sources::BUILTINS,
    );
    let source = mir.sources.add("binary.yaml", "value: !!binary SGk=\n");
    let plan = data_plan::parse_registered(&mir.sources, source, Format::Yaml).unwrap();
    let date = mir.sources.add("date.toml", "value = 2026-09-12\n");
    let date_plan = data_plan::parse_registered(&mir.sources, date, Format::Toml).unwrap();
    let sealed = mir.seal().unwrap();
    let contract = DataContract::from_mir(&sealed).unwrap();
    let mut rt = Runtime::new(&sealed).unwrap();
    let data = rt.materialize_data(&contract, &plan).unwrap();
    let dict = rt.enum_payload(&data).unwrap().unwrap().to_owned();
    let bytes = rt.dict_entry(&dict, 0).unwrap().1.to_owned();
    let bytes = rt.enum_payload(&bytes).unwrap().unwrap().to_owned();
    let slice = rt.bytes_slice(&bytes, 1, 2, bytes.location()).unwrap();
    assert!(rt.bytes_slice(&bytes, 2, 3, bytes.location()).is_err());
    let date = rt.materialize_data(&contract, &date_plan).unwrap();
    assert_eq!(
        rt.semantic_json(&contract, &data).unwrap_err(),
        "JSON cannot encode Bytes"
    );
    assert_eq!(
        rt.semantic_json(&contract, &date).unwrap_err(),
        "JSON cannot encode temporal values; use a codec first"
    );
    let roots = rt.publish(&[data, slice, date]).unwrap();
    assert_eq!(rt.bytes_data(&roots[1]).unwrap(), b"i");
    assert_eq!(rt.main.bytes.entries.len(), 1);
    assert!(rt.bytes_data(&bytes).is_err());
    let dict = rt.enum_payload(&roots[2]).unwrap().unwrap().to_owned();
    let date = rt.dict_entry(&dict, 0).unwrap().1.to_owned();
    assert_eq!(
        rt.variant_name(date.type_id(), rt.enum_tag(&date).unwrap())
            .unwrap(),
        "LocalDate"
    );
}

#[test]
fn initialization_demands_cache_publish_and_report_cycles_once() {
    let mir = graph(SOURCE);
    let key = DemandKey::Export(
        *mir.exports
            .iter()
            .flatten()
            .find(|id| mir.symbols[id.index()].name == "text")
            .unwrap(),
    );
    let ty = value_type(&mir, "text");
    let mut rt = Runtime::new(&mir.seal().unwrap()).unwrap();
    rt.register_demand(key, ty).unwrap();
    assert!(rt.publish(&[]).is_err());
    assert!(matches!(rt.begin_demand(key).unwrap(), Demand::Evaluate));
    let text = rt
        .string(ty, [1, 2, 3], "initialization shared long string")
        .unwrap();
    rt.complete_demand(key, text.clone()).unwrap();
    assert!(matches!(rt.begin_demand(key).unwrap(), Demand::Ready(_)));
    let roots = rt.publish(&[text]).unwrap();
    let Demand::Ready(cached) = rt.begin_demand(key).unwrap() else {
        panic!("demand lost at publication")
    };
    assert_eq!(cached.words(), roots[0].words());
    assert_eq!(rt.main.strings.entries.len(), 1);
    assert_eq!(
        rt.text(cached.as_ref()).unwrap().as_str(),
        "initialization shared long string"
    );
    assert!(rt.register_demand(key, ty).is_err());

    let mut rt = Runtime::new(&mir.seal().unwrap()).unwrap();
    rt.register_demand(key, ty).unwrap();
    assert!(matches!(rt.begin_demand(key).unwrap(), Demand::Evaluate));
    assert!(
        rt.begin_demand(key)
            .unwrap_err()
            .contains("dependency cycle")
    );
    assert!(matches!(rt.begin_demand(key).unwrap(), Demand::Failed));
    rt.fail_demand(key).unwrap();
    assert!(rt.publish(&[]).is_err());
    assert!(!rt.published);
}

#[test]
fn closure_environments_publish_nested_captures_and_shared_objects() {
    let mir = graph(SOURCE);
    let mut rt = Runtime::new(&mir.seal().unwrap()).unwrap();
    let ty = value_type(&mir, "function");
    let text = rt
        .string(
            value_type(&mir, "text"),
            [1, 2, 3],
            "shared captured string longer than inline",
        )
        .unwrap();
    let empty = rt.closure(ty, [1, 3, 4], 0, &[]).unwrap();
    assert!(rt.capture(&empty, 0).is_err());
    let inner = rt.closure(ty, [1, 4, 5], 17, &[text.clone()]).unwrap();
    let outer = rt
        .closure(ty, [1, 5, 6], 23, &[inner.clone(), text.clone(), empty])
        .unwrap();
    let published = rt.publish(&[outer, inner.clone(), text]).unwrap();
    assert_eq!(rt.main.environments.entries.len(), 2);
    assert_eq!(rt.main.strings.entries.len(), 1);
    assert!(rt.work.environments.entries.is_empty());
    assert!(rt.capture(&inner, 0).is_err());
    let nested = rt.capture(&published[0], 0).unwrap().to_owned();
    assert_eq!(nested.words(), published[1].words());
    assert_eq!(rt.function_id(&nested).unwrap(), 17);
    let captured = rt.capture(&nested, 0).unwrap();
    assert_eq!(captured.words(), published[2].words());
    assert_eq!(captured.location(), [1, 2, 3]);
    assert_eq!(
        rt.function_id(&rt.capture(&published[0], 2).unwrap().to_owned())
            .unwrap(),
        0
    );
    assert!(rt.capture(&published[0], 3).is_err());
}

#[test]
fn native_tables_publish_aliases_once_and_reject_stale_work_values() {
    let mir = graph(SOURCE);
    let mut rt = Runtime::new(&mir.seal().unwrap()).unwrap();
    let int = value_type(&mir, "integer");
    let string = value_type(&mir, "text");
    let array_ty = value_type(&mir, "ints");
    let dict_ty = value_type(&mir, "mapping");
    let tuple_ty = value_type(&mir, "repeated");
    let one = rt.scalar(int, [1, 0, 1], 7).unwrap();
    let array = rt.array(array_ty, [1, 1, 2], &[one]).unwrap();
    let slice = rt.slice(&array, 0, 1, [1, 2, 3]).unwrap();
    let k1 = rt
        .string(string, [1, 3, 4], "z long dictionary key")
        .unwrap();
    let k2 = rt
        .string(string, [1, 4, 5], "a long dictionary key")
        .unwrap();
    let dict = rt
        .dict(
            dict_ty,
            [1, 5, 6],
            &[(k1, slice.clone()), (k2, array.clone())],
        )
        .unwrap();
    let tuple = rt
        .aggregate(tuple_ty, [1, 6, 7], &[array.clone(), slice])
        .unwrap();
    let roots = rt.publish(&[dict, tuple]).unwrap();
    assert!(rt.work.arrays.entries.is_empty());
    assert_eq!(rt.main.arrays.entries.len(), 3); // keys, values, one shared backing
    assert_eq!(rt.main.records.entries.len(), 1);
    assert!(rt.array_len(&array).is_err()); // stale initialize handle
    let (key, found) = rt.dict_entry(&roots[0], 0).unwrap();
    assert_eq!(rt.text(key).unwrap().as_str(), "a long dictionary key");
    assert_eq!(
        HeapRef::from_raw(found.words()[2] as u32).world(),
        World::Main
    );
    let tuple_field = rt.field(&roots[1], 0).unwrap();
    assert_eq!(tuple_field.words()[2] as u32, found.words()[2] as u32);
    assert_eq!(
        rt.scalar_bits(rt.array_get(&found.to_owned(), 0).unwrap())
            .unwrap(),
        7
    );
    assert_eq!(
        rt.array_get(&found.to_owned(), 0).unwrap().location(),
        [1, 0, 1]
    );
    let new_array = rt
        .array(
            array_ty,
            [1, 7, 8],
            &[rt.scalar(int, [1, 8, 9], 9).unwrap()],
        )
        .unwrap();
    assert_eq!(
        HeapRef::from_raw(new_array.words[2] as u32).world(),
        World::Work
    );
    assert!(rt.publish(&roots).is_err());
}

#[test]
fn publication_failure_keeps_initialize_world_and_main_unpublished() {
    let mir = graph(SOURCE);
    let mut rt = Runtime::new(&mir.seal().unwrap()).unwrap();
    let array_ty = value_type(&mir, "ints");
    let array = rt.array(array_ty, [1, 0, 1], &[]).unwrap();
    let mut broken = array.clone();
    broken.words[3] = 1; // invalid slice beyond empty backing
    let identity = rt.identity();
    assert!(rt.publish(&[array.clone(), broken]).is_err());
    assert_eq!(rt.identity(), identity);
    assert!(!rt.published);
    assert!(rt.main.arrays.entries.is_empty());
    assert_eq!(rt.array_len(&array).unwrap(), 0);
    let roots = rt.publish(&[array]).unwrap();
    assert_eq!(rt.array_len(&roots[0]).unwrap(), 0);
}

#[test]
fn record_and_tuple_share_slots_and_updates_keep_original_values() {
    let mir = graph(SOURCE);
    let mut rt = Runtime::new(&mir.seal().unwrap()).unwrap();
    let int = value_type(&mir, "integer");
    let string = value_type(&mir, "text");
    let rec = value_type(&mir, "record");
    let pair = value_type(&mir, "pair");
    let n = rt.scalar(int, [1, 0, 1], 42).unwrap();
    let text = rt
        .string(string, [1, 1, 2], "shared long string object")
        .unwrap();
    let tuple = rt
        .aggregate(pair, [1, 2, 3], &[n.clone(), text.clone()])
        .unwrap();
    let record = rt.aggregate(rec, [1, 3, 4], &[n, text]).unwrap();
    assert_ne!(tuple.words[2], record.words[2]);
    assert_eq!(rt.work.records.entries.len(), 2);
    let replacement = rt.scalar(int, [1, 4, 5], 99).unwrap();
    let updated = rt
        .replace_field(&record, 0, &replacement, [1, 5, 6])
        .unwrap();
    assert_eq!(rt.scalar_bits(rt.field(&record, 0).unwrap()).unwrap(), 42);
    assert_eq!(rt.scalar_bits(rt.field(&updated, 0).unwrap()).unwrap(), 99);
    assert_eq!(rt.work.strings.entries.len(), 1);
}
