use super::*;

#[test]
fn literal_pair_scalar_replacement_preserves_sharing_order_and_gc() {
    let source = r#"
use std::dict as d;
use std::codec as c;
use std::value::{Value};
type Box = enum { Object(Dict(Int)) };
def boxed: Fn(Array((String,Int))) -> Box = fn(pairs) { Box::Object(d::from_pairs(pairs)) };
def unknown: Fn(Array((String,Int))) -> Dict(Int) = fn(pairs) { d::from_pairs(pairs) };
pub def answer: Fn() -> Value = fn() {
    let original = [("z", 3), ("a", 1)];
    let dynamic = "b";
    c::encode((boxed([("z", 3), ("a", 1)]), d::from_pairs([("z", 3), ("a", 1)]),
        d::from_pairs(original), original, unknown([("x", 7)]),
        d::from_pairs([(dynamic, 5)])))
};
"#;
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 32 * 1024 * 1024,
        })
        .unwrap();
    session.initialize().unwrap();
    let first = session.call(&[]).unwrap();
    assert_eq!(first[1], serde_json::json!({"a":1,"z":3}));
    assert_eq!(first[2], first[1]);
    assert_eq!(first[3], serde_json::json!([["z", 3], ["a", 1]]));
    assert_eq!(first[4], serde_json::json!({"x":7}));
    assert_eq!(first[5], serde_json::json!({"b":5}));
    session.collect_garbage().unwrap();
    assert_eq!(session.call(&[]).unwrap(), first);
}

#[test]
fn literal_pair_duplicates_keep_runtime_diagnostics() {
    let source = r#"use std::dict as d;
pub def answer: Fn() -> Dict(Int) = fn() { d::from_pairs([("x",1),("x",2)]) };"#;
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let error = session.call(&[]).unwrap_err();
    assert!(error.contains("duplicate field"), "{error}");
}

#[test]
fn literal_pair_lowering_keeps_dictionary_key_value_and_constructor_origins() {
    let source = r#"use std::dict as d;
type Box = enum { Object(Dict(Int)) };
def boxed: Fn(Array((String,Int))) -> Box = fn(pairs) { Box::Object(d::from_pairs(pairs)) };
pub def answer: Fn() -> Box = fn() { boxed([("z", 23), ("a", 17)]) };"#;
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.initialize().unwrap();
    let closure = crate::transport::Value {
        pointer: session.entry().unwrap(),
        ty: session.manifest.entry_type,
    };
    let mut result = session.invoke_values(closure, &[]).unwrap();
    for _ in 0..2 {
        let (_, dict) = session.variant(result).unwrap();
        let dict = dict.unwrap();
        let output = session.output();
        let dictionary_origin = output.location_words(dict.pointer as u64).unwrap();
        assert_eq!(dictionary_origin[1], 3);
        let (_, values) = session
            .dict_items(dict)
            .unwrap()
            .into_iter()
            .find(|(key, _)| key == "z")
            .unwrap();
        let value_origin = session
            .output()
            .location_words(values.pointer as u64)
            .unwrap();
        assert_eq!(value_origin[1], 4);
        // The enum head retains the constructor materialization in boxed's body.
        let constructor_origin = session
            .output()
            .location_words(result.pointer as u64)
            .unwrap();
        assert_eq!(constructor_origin[1], 3);
        result = session.collect_work(&[result]).unwrap().0[0];
    }
}

#[test]
fn literal_pair_lowering_preserves_debug_order_and_first_failure() {
    let source = r#"use std::dict as d;
type Box = enum { Object(Dict(Int)) };
def boxed: Fn(Array((String,Int))) -> Box = fn(pairs) { Box::Object(d::from_pairs(pairs)) };
pub def answer: Fn() -> Box = fn() { boxed([("z",dbg!(3)),("a",dbg!(1))]) };"#;
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    session.call(&[]).unwrap();
    assert_eq!(
        session
            .take_debug_events()
            .unwrap()
            .iter()
            .map(|e| e.repr.clone())
            .collect::<Vec<_>>(),
        ["3", "1"]
    );
    let failure = source.replace("dbg!(3)", "fail!(\"first failure\")");
    let bytes = compile(&failure).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    assert!(session.call(&[]).unwrap_err().contains("first failure"));
    assert!(session.take_debug_events().unwrap().is_empty());
}
