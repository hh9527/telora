use super::*;
fn call(source: &str) -> serde_json::Value {
    let bytes = compile(source).unwrap();
    let mut s = crate::session::Session::load(&bytes, 1_000_000_000).unwrap();
    s.configure_gc(crate::gc::GcConfig {
        allocation_interval: 1,
        heap_limit: 32 * 1024 * 1024,
    })
    .unwrap();
    s.set_debug_enabled(true).unwrap();
    s.initialize().unwrap();
    let first = s.call(&[]);
    assert!(
        first.is_ok(),
        "first={first:?} events={:?}",
        s.take_debug_events()
    );
    let value = first.unwrap();
    s.collect_garbage().unwrap();
    let again = s.call(&[]);
    assert!(
        again.is_ok(),
        "again={again:?}; diagnostics={:?}",
        (s.diagnostics(), s.take_debug_events())
    );
    assert_eq!(again.unwrap(), value);
    value
}
#[test]
fn column_map_updates_preserve_old_versions_and_dict_alias_identity() {
    assert_eq!(
        call(
            r#"
use std::map as m;use std::dict as d;
pub def answer:Fn()->(Bool,Dict(Int),Array((Int,String)),Array((Int,String)),Option(String),Int) = fn(){
 let a=m::from_pairs([(3,"three"),(1,"one"),(2,"two"),(1,"ONE")]);
 let b=m::remove(m::insert(a,2,"TWO"),3);
 let dict:Map(String,Int)={x:7};
 (Map(String,Int).type==Dict(Int).type,d::merge(dict,{y:8}),m::pairs(a),m::pairs(b),m::get(b,2),m::length(a))
};"#
        ),
        serde_json::json!([true,{"x":7,"y":8},[[1,"ONE"],[2,"two"],[3,"three"]],[[1,"ONE"],[2,"TWO"]],"TWO",3])
    );
}
#[test]
fn column_map_custom_keys_and_indexing() {
    assert_eq!(
        call(
            r#"
use std::map as m;
type Key = struct(Int);
pub def answer:Fn()->(String,Array((Key,String)),Array((Key,String)))=fn(){
 let a=m::from_pairs([(Key(3),"three"),(Key(1),"one")]);
 let b=m::insert(a,Key(2),"two");
 (b[Key(2)],m::pairs(a),m::pairs(b))
};"#
        ),
        serde_json::json!([
            "two",
            [[1, "one"], [3, "three"]],
            [[1, "one"], [2, "two"], [3, "three"]]
        ])
    );
}
#[test]
fn column_map_pipeline_builder_handles_duplicate_keys_and_gc() {
    assert_eq!(
        call(
            r#"
use std::pipeline as p;use std::map as m;use std::array as a;
pub def answer:Fn()->(Array((Int,String)),Array((Int,String)))=fn(){
 let source=p::from_array([3,1,2,1,5,4]);
 let result=p::run(source |> p::collect_map(fn(n){n},fn(n){`value\{n}`}));
 let other=m::merge(result,m::from_pairs([(1,"replacement"),(6,"six")]));
 (m::pairs(result),m::pairs(other))
};"#
        ),
        serde_json::json!([
            [
                [1, "value1"],
                [2, "value2"],
                [3, "value3"],
                [4, "value4"],
                [5, "value5"]
            ],
            [
                [1, "replacement"],
                [2, "value2"],
                [3, "value3"],
                [4, "value4"],
                [5, "value5"],
                [6, "six"]
            ]
        ])
    );
}
#[test]
fn column_map_empty_and_product_keys() {
    assert_eq!(
        call(
            r#"
use std::map as m;
pub def answer:Fn()->(Array((Int,String)),Option(String),Array(((Int,String),Int)),Bool)=fn(){
 let empty:Map(Int,String)=m::empty();
 let a=m::from_pairs([((2,"a"),1),((1,"b"),2),((1,"a"),3)]);
 (m::pairs(empty),m::get(empty,0),m::pairs(a),a==m::from_pairs([((1,"a"),3),((1,"b"),2),((2,"a"),1)]))
};"#
        ),
        serde_json::json!([
            [],
            null,
            [[[1, "a"], 3], [[1, "b"], 2], [[2, "a"], 1]],
            true
        ])
    );
}

#[test]
fn column_newtype_constructor_gc_control() {
    assert_eq!(
        call(
            r#"type Key=struct(Int);pub def answer:Fn()->(Int,Int,Int)=fn(){let a=Key(1);let b=Key(2);let ignored=dbg!(b.0,"b");(a.0,b.0,Key(3).0)};"#
        ),
        serde_json::json!([1, 2, 3])
    );
}

#[test]
fn column_map_staged_publication_restores_generic_columns() {
    let mir = graph(
        r#"
use std::build as build;use std::map as m;use std::pipeline as p;use std::codec as codec;
type Key=struct(Int);
pub def answer:Fn(Int)->Option(String)=build::stage(
 fn(){let values=p::run(p::from_array([3,1,2,1]) |> p::collect_map(fn(n){Key(n)},fn(n){`value\{n}`}));{runtime:values,information:{ok:codec::encode(True)}}},
 fn(values){fn(n){m::get(values,Key(n))}}
);
"#,
    );
    let symbol = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .unwrap();
    let executable = mir.seal_export(symbol).unwrap();
    let bytes = crate::staged_build::StagedBuild::compile(&executable)
        .unwrap()
        .publish(500_000_000, 64 * 1024 * 1024)
        .unwrap();
    for _ in 0..2 {
        let mut s = crate::staged_build::load(&bytes, 500_000_000, 64 * 1024 * 1024).unwrap();
        s.configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 32 * 1024 * 1024,
        })
        .unwrap();
        for n in [1, 3, 9, 2] {
            assert_eq!(
                s.call(&[n.into()]).unwrap(),
                if n == 9 {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(format!("value{n}"))
                }
            );
            s.collect_garbage().unwrap();
        }
    }
}
#[test]
fn column_map_mixed_operations_match_btreemap() {
    let bytes=compile(r#"
use std::map as m;use std::array as a;
pub def answer:Fn(Array((Int,Int,Int)))->Array((Int,Int))=fn(operations){
 m::pairs(a::fold(operations,m::empty(),fn(map,op){if op.0==0{m::insert(map,op.1,op.2)}else{m::remove(map,op.1)}}))
};"#).unwrap();
    let mut s = crate::session::Session::load(&bytes, 2_000_000_000).unwrap();
    s.initialize().unwrap();
    let mut seed = 17u64;
    let mut reference = std::collections::BTreeMap::<i64, i64>::new();
    let mut ops = Vec::new();
    for i in 0..512 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let key = ((seed >> 32) % 97) as i64 - 48;
        let remove = i % 4 == 0;
        if remove {
            reference.remove(&key);
        } else {
            reference.insert(key, i);
        }
        ops.push(serde_json::json!([i64::from(remove), key, i]));
    }
    let expected: Vec<_> = reference
        .into_iter()
        .map(|(k, v)| serde_json::json!([k, v]))
        .collect();
    assert_eq!(
        s.call(&[serde_json::json!(ops)]).unwrap(),
        serde_json::json!(expected)
    );
}

#[test]
fn column_map_empty_uninhabited_and_unit_keys() {
    assert_eq!(
        call(
            r#"use std::map as m;pub def answer:Fn()->(Int,Int,Int)=fn(){let a:Map(Int,Never)=m::empty();let b:Map(Never,Int)=m::empty();let c=m::from_pairs([((),1),((),2)]);(m::length(a),m::length(b),c[()])};"#
        ),
        serde_json::json!([0, 0, 2])
    );
}
