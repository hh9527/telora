use super::*;
use crate::test_support::{graph, value_type};

const SOURCE: &str = include_str!("../../tests/fixtures/runtime.telora");

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
