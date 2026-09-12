use super::*;
use crate::mir::{Mir, ResolvedType};
const INT: TypeId = TypeId(0);
const STRING: TypeId = TypeId(1);
const ARRAY: TypeId = TypeId(2);
const TUPLE: TypeId = TypeId(3);
const RECORD: TypeId = TypeId(4);
const DICT: TypeId = TypeId(5);
const TEXTS: TypeId = TypeId(6);
const TUPLES: TypeId = TypeId(7);
const DICT_ARRAY: TypeId = TypeId(8);
const EMPTY_ARRAY: TypeId = TypeId(10);
const EMPTY_DICT: TypeId = TypeId(11);
const UNIT: TypeId = TypeId(12);
fn arena() -> Arena {
    let definitions = vec![
        (T::Int, vec![]),
        (T::String, vec![]),
        (T::Array, vec![0]),
        (T::Tuple, vec![0, 1]),
        (T::Record(vec!["count".into(), "label".into()]), vec![0, 1]),
        (T::Dict, vec![0]),
        (T::Array, vec![1]),
        (T::Array, vec![3]),
        (T::Dict, vec![2]),
        (T::Never, vec![]),
        (T::Array, vec![9]),
        (T::Dict, vec![9]),
        (T::Tuple, vec![]),
    ];
    let mut mir = Mir {
        symbols_closed: true,
        types_solved: true,
        types: definitions
            .into_iter()
            .map(|(constructor, arguments)| ResolvedType {
                constructor,
                arguments: arguments.into_iter().map(TypeId).collect(),
            })
            .collect(),
        ..Mir::default()
    };
    mir.type_layouts.resize(mir.types.len(), None);
    Arena::new(&mir.seal().expect("closed test type graph")).unwrap()
}
#[test]
fn tuple_and_record_share_table_but_keep_types_and_sources() {
    let mut a = arena();
    let number = a.scalar(INT, [1, 20, 21], 42).unwrap();
    let label = a
        .string(STRING, [2, 30, 50], "a long shared string payload")
        .unwrap();
    let fields = vec![number.clone(), label.clone()];
    let tuple = a.aggregate(TUPLE, [3, 0, 60], &fields).unwrap();
    let record = a.aggregate(RECORD, [4, 0, 60], &fields).unwrap();
    assert_eq!(tuple.words.len(), 3);
    assert_eq!(record.words.len(), 3);
    assert_eq!(tuple.words[2], 0);
    assert_eq!(record.words[2], 1);
    assert_eq!(tuple.type_id(), TUPLE);
    assert_eq!(record.type_id(), RECORD);
    assert_eq!(a.records.entries.len(), 2);
    assert_eq!(a.records.get(0).unwrap(), a.records.get(1).unwrap());
    assert_eq!(a.records.get(0).unwrap().len() * 8, 56);
    assert_eq!(a.field(&tuple, 0).unwrap().location(), [1, 20, 21]);
    assert_eq!(
        a.text(a.field(&record, 1).unwrap()).unwrap().as_str(),
        "a long shared string payload"
    );
    assert_eq!(a.field(&record, 1).unwrap().location(), [2, 30, 50]);
    assert_eq!(a.strings.entries.len(), 1);
    let replacement = a.scalar(INT, [8, 9, 10], 99).unwrap();
    let updated = a
        .replace_field(&tuple, 0, &replacement, [5, 0, 60])
        .unwrap();
    assert_eq!(a.scalar_bits(a.field(&tuple, 0).unwrap()).unwrap(), 42);
    assert_eq!(a.scalar_bits(a.field(&updated, 0).unwrap()).unwrap(), 99);
    assert_eq!(
        a.field(&updated, 1).unwrap().words(),
        label.as_ref().words()
    );
    assert_eq!(a.strings.entries.len(), 1); // only descriptors copied
    assert!(a.aggregate(TUPLE, [0; 3], &[label, number]).is_err());
    assert!(a.field(&record, 2).is_err());
    let before = a.records.entries.len();
    let unit = a.aggregate(UNIT, [9, 10, 11], &[]).unwrap();
    assert_eq!(unit.as_ref().words().len(), 2);
    assert_eq!(unit.location(), [9, 10, 11]);
    assert_eq!(a.records.entries.len(), before);
}
#[test]
fn slices_share_backing_and_updates_preserve_original_values() {
    let mut a = arena();
    let values = (0..4)
        .map(|i| a.scalar(INT, [1, i, i + 1], i as u64).unwrap())
        .collect::<Vec<_>>();
    let array = a.array(ARRAY, [2, 0, 4], &values).unwrap();
    assert_eq!(array.words.len(), 4);
    assert_eq!(a.arrays.get(0).unwrap().len(), 12);
    let slice = a.slice(&array, 1, 4, [3, 0, 3]).unwrap();
    let nested = a.slice(&slice, 1, 2, [4, 0, 1]).unwrap();
    assert_eq!(a.arrays.entries.len(), 1);
    assert_eq!(a.array_len(&nested).unwrap(), 1);
    assert_eq!(a.scalar_bits(a.array_get(&nested, 0).unwrap()).unwrap(), 2);
    assert_eq!(a.array_get(&nested, 0).unwrap().location(), [1, 2, 3]);
    assert_eq!(nested.location(), [4, 0, 1]);
    let replacement = a.scalar(INT, [9, 0, 1], 99).unwrap();
    let updated = a.array_set(&slice, 1, &replacement, [5, 0, 3]).unwrap();
    assert_eq!(a.scalar_bits(a.array_get(&array, 2).unwrap()).unwrap(), 2);
    assert_eq!(
        a.scalar_bits(a.array_get(&updated, 1).unwrap()).unwrap(),
        99
    );
    assert_eq!(a.array_get(&updated, 0).unwrap().location(), [1, 1, 2]);
    assert!(a.array_get(&nested, 1).is_err());
    assert!(a.slice(&slice, 2, 1, [0; 3]).is_err());
    assert!(a.slice(&slice, 0, 4, [0; 3]).is_err());
    let empty = a.array(EMPTY_ARRAY, [0; 3], &[]).unwrap();
    assert_eq!(a.array_len(&empty).unwrap(), 0);
    assert!(a.array_get(&empty, 0).is_err());
}
#[test]
fn strings_and_nested_aggregates_use_fixed_element_stride_without_deep_copy() {
    let mut a = arena();
    let short = a.string(STRING, [1, 0, 6], "你好").unwrap();
    let long = a
        .string(STRING, [2, 0, 30], "a sufficiently long heap string")
        .unwrap();
    let array = a
        .array(TEXTS, [3, 0, 2], &[short.clone(), long.clone()])
        .unwrap();
    assert_eq!(a.arrays.get(0).unwrap().len() * 8, 64);
    assert_eq!(
        a.text(a.array_get(&array, 0).unwrap()).unwrap().as_str(),
        "你好"
    );
    assert_eq!(
        a.text(a.array_get(&array, 1).unwrap()).unwrap().as_str(),
        "a sufficiently long heap string"
    );
    assert_eq!(a.strings.entries.len(), 1);
    let int = a.scalar(INT, [0; 3], 1).unwrap();
    let tuple = a.aggregate(TUPLE, [4, 0, 2], &[int, long]).unwrap();
    let nested = a.array(TUPLES, [5, 0, 1], &[tuple.clone()]).unwrap();
    assert_eq!(
        a.array_get(&nested, 0).unwrap().words(),
        tuple.as_ref().words()
    );
    assert_eq!(a.records.entries.len(), 1);
    assert_eq!(a.strings.entries.len(), 1);
    assert!(a.array(ARRAY, [0; 3], &[short]).is_err());
}
#[test]
fn dictionary_sorts_columns_and_preserves_sources_on_updates() {
    let mut a = arena();
    let key = a.string(STRING, [1, 0, 1], "a").unwrap();
    let collision = a.string(STRING, [2, 0, 1], "q").unwrap();
    let one = a.scalar(INT, [3, 0, 1], 1).unwrap();
    let two = a.scalar(INT, [4, 0, 1], 2).unwrap();
    let dict = a
        .dict(
            DICT,
            [5, 0, 2],
            &[(collision.clone(), two.clone()), (key.clone(), one.clone())],
        )
        .unwrap();
    assert_eq!(a.dict_len(&dict).unwrap(), 2);
    assert_eq!(dict.words.len(), 4);
    assert_eq!(a.arrays.get(dict.words[2] as u32).unwrap().len() * 8, 64);
    assert_eq!(a.arrays.get(dict.words[3] as u32).unwrap().len() * 8, 48);
    assert_eq!(
        a.text(a.dict_entry(&dict, 0).unwrap().0).unwrap().as_str(),
        "a"
    );
    assert_eq!(
        a.scalar_bits(a.dict_get(&dict, &collision).unwrap().unwrap())
            .unwrap(),
        2
    );
    let same_key = a.string(STRING, [8, 0, 1], "a").unwrap();
    let updated = a.dict_insert(&dict, &same_key, &two, [6, 0, 2]).unwrap();
    assert_eq!(a.dict_len(&updated).unwrap(), 2);
    assert_eq!(a.dict_entry(&updated, 0).unwrap().0.location(), [1, 0, 1]);
    assert_eq!(
        a.dict_get(&updated, &key).unwrap().unwrap().location(),
        [4, 0, 1]
    );
    assert_eq!(
        a.scalar_bits(a.dict_get(&dict, &key).unwrap().unwrap())
            .unwrap(),
        1
    );
    let removed = a.dict_remove(&updated, &key, [7, 0, 1]).unwrap();
    assert_eq!(a.dict_len(&removed).unwrap(), 1);
    assert!(a.dict_get(&removed, &key).unwrap().is_none());
    assert_eq!(
        a.text(a.dict_entry(&removed, 0).unwrap().0)
            .unwrap()
            .as_str(),
        "q"
    );
    let empty = a.dict(EMPTY_DICT, [0; 3], &[]).unwrap();
    assert_eq!(a.dict_len(&empty).unwrap(), 0);
    assert!(a.dict(DICT, [0; 3], &[(key, same_key)]).is_err());
}
#[test]
fn dictionary_order_is_independent_of_construction_and_search_handles_boundaries() {
    let mut a = arena();
    let mut pairs = Vec::new();
    for (i, text) in ["", "a", "aa", "z", "é", "中"].iter().enumerate() {
        pairs.push((
            a.string(STRING, [0; 3], text).unwrap(),
            a.scalar(INT, [0; 3], i as u64).unwrap(),
        ));
    }
    let forward = a.dict(DICT, [0; 3], &pairs).unwrap();
    pairs.reverse();
    let reverse = a.dict(DICT, [0; 3], &pairs).unwrap();
    for i in 0..pairs.len() {
        let (key, value) = a.dict_entry(&forward, i).unwrap();
        let (other_key, other_value) = a.dict_entry(&reverse, i).unwrap();
        assert_eq!(key.words(), other_key.words());
        assert_eq!(value.words(), other_value.words());
        assert_eq!(
            a.dict_get(&reverse, &key.to_owned())
                .unwrap()
                .unwrap()
                .words(),
            value.words()
        );
    }
    for text in ["ab", "zz", "末尾"] {
        let key = a.string(STRING, [0; 3], text).unwrap();
        assert!(a.dict_get(&forward, &key).unwrap().is_none());
    }
    let key = a.string(STRING, [0; 3], "b").unwrap();
    let value = a.scalar(INT, [0; 3], 99).unwrap();
    let inserted = a.dict_insert(&forward, &key, &value, [0; 3]).unwrap();
    assert_eq!(
        a.text(a.dict_entry(&inserted, 3).unwrap().0)
            .unwrap()
            .as_str(),
        "b"
    );
    assert_eq!(a.dict_len(&forward).unwrap(), 6);
    let empty = a.dict(EMPTY_DICT, [0; 3], &[]).unwrap();
    assert!(a.dict_get(&empty, &key).unwrap().is_none());
    assert!(a.dict_entry(&empty, 0).is_err());
    let before = a.arrays.entries.len();
    assert!(a.dict(EMPTY_DICT, [0; 3], &[(key, value)]).is_err());
    assert_eq!(a.arrays.entries.len(), before);
}
#[test]
fn dictionary_references_arrays_and_heap_strings_without_copying_their_contents() {
    let mut a = arena();
    let k1 = a
        .string(STRING, [1, 0, 40], "long keys compare by their contents")
        .unwrap();
    let k2 = a
        .string(STRING, [2, 0, 40], "long keys compare by their contents")
        .unwrap();
    let one = a.scalar(INT, [0; 3], 1).unwrap();
    let array = a.array(ARRAY, [3, 0, 1], &[one]).unwrap();
    let dict = a
        .dict(DICT_ARRAY, [4, 0, 1], &[(k1, array.clone())])
        .unwrap();
    let found = a.dict_get(&dict, &k2).unwrap().unwrap();
    assert_eq!(found.words(), array.as_ref().words());
    let cloned = found.to_owned();
    assert_eq!(a.scalar_bits(a.array_get(&cloned, 0).unwrap()).unwrap(), 1);
    assert_eq!(a.arrays.entries.len(), 3); // original array plus two dictionary columns
    assert_eq!(a.strings.entries.len(), 2);
}
#[test]
fn cross_arena_values_are_rejected_before_any_table_write() {
    let a = arena();
    let mut b = arena();
    let value = a.scalar(INT, [0; 3], 1).unwrap();
    assert!(
        b.array(ARRAY, [0; 3], &[value])
            .unwrap_err()
            .contains("different arena")
    );
    assert!(b.arrays.entries.is_empty());
}

#[test]
fn slot_table_growth_preserves_object_buffers_and_heap_ids() {
    let mut words = WordTable::default();
    let payload = vec![11, 22, 33];
    let original_buffer = payload.as_ptr();
    let id = words.push(payload).unwrap();
    assert_eq!(words.get(id).unwrap().as_ptr(), original_buffer);
    let mut strings = RawStringTable::default();
    let string_id = strings.push(b"retained object content").unwrap();
    let string_buffer = strings.get(string_id).unwrap().as_ptr();
    let old_words_capacity = words.entries.capacity();
    let old_strings_capacity = strings.entries.capacity();
    for i in 0..old_words_capacity.max(old_strings_capacity) + 16 {
        words.push(vec![i as u64; 4]).unwrap();
        strings
            .push(b"another separately allocated object")
            .unwrap();
    }
    assert!(words.entries.capacity() > old_words_capacity);
    assert!(strings.entries.capacity() > old_strings_capacity);
    assert_eq!(words.get(id).unwrap(), [11, 22, 33]);
    assert_eq!(words.get(id).unwrap().as_ptr(), original_buffer);
    assert_eq!(strings.get(string_id).unwrap(), b"retained object content");
    assert_eq!(strings.get(string_id).unwrap().as_ptr(), string_buffer);
}
