//! Only built by the explicitly enabled handwritten mutation experiment.
use super::*;

#[test]
fn handwritten_mut_buffers_preserve_inputs_and_survive_growth_gc_and_cycles() {
    let source = r#"
use std::pipeline as p;
use std::array as a;
use std::codec as c;
use std::type_desc as td;
use std::value::{Value};
type First = struct(String);
type Second = struct(String);
pub def answer: Fn(Array(String)) -> Value = fn(input) {
    let types = [First.type, First.type, Second.type];
    let names = fn(types) { a::map(types, fn(ty) { td::name(ty) }) };
    let unique = names(p::oracle_type_unique(types));
    let flags = [unique == [td::name(First.type), td::name(Second.type)],
        names(p::oracle_type_unique(types)) == unique,
        names(p::oracle_type_discover([First.type], fn(ty) { match td::name(ty) { Some(name) => name, None => "" } },
            fn(ty) { if td::name(ty) == td::name(First.type) { [Second.type, First.type].ty!(Array(Type)) } else { [First.type].ty!(Array(Type)) } })) == unique];
    let output = p::oracle_array_new();
    let index = p::oracle_dict_new();
    let text = p::oracle_string_new();
    a::fold(input, False, fn(previous,item) {
        p::oracle_array_push(output, item);
        p::oracle_dict_put(index, item, [item]);
        p::oracle_string_push(text, item)
    });
    let discovered = p::oracle_discover([1], fn(n) { `\{n}` }, fn(n) {
        if n < 4 { [n + 1, 1] } else { [1] }
    });
    c::encode((input, p::oracle_array_finish(output), p::oracle_dict_finish(index),
        p::oracle_string_finish(text), discovered, flags))
};
"#;
    let bytes = compile(source).unwrap();
    for count in [0, 1, 2, 3, 17, 65] {
        let input: Vec<_> = (0..count).map(|i| format!("key-{}", i % 9)).collect();
        let object: serde_json::Map<_, _> = input
            .iter()
            .map(|s| (s.clone(), serde_json::json!([s])))
            .collect();
        let expected = serde_json::json!([
            input,
            input,
            object,
            input.concat(),
            [1, 2, 3, 4],
            [true, true, true]
        ]);
        let mut s = crate::session::Session::load(&bytes, 5_000_000_000).unwrap();
        s.configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 32 * 1024 * 1024,
        })
        .unwrap();
        s.initialize().unwrap();
        assert_eq!(s.call(&[serde_json::json!(input)]).unwrap(), expected);
        s.collect_garbage().unwrap();
        assert_eq!(s.call(&[serde_json::json!(input)]).unwrap(), expected);
    }
}

#[test]
fn type_visited_state_cannot_be_imported_by_applications() {
    for name in ["OracleTypeSet", "oracle_type_set", "oracle_type_first"] {
        let source = format!("use std::pipeline::{{{name}}}; pub def answer: Int = 1;");
        let inventory = std::iter::once("@src/main")
            .chain(static_sources::BUILTINS.iter().map(|(name, _)| *name))
            .map(|name| ModuleSpec {
                name: name.into(),
                kind: telora_core::mir::ModuleKind::Source,
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
                source.clone()
            } else {
                static_sources::BUILTINS
                    .iter()
                    .find(|(module, _)| *module == name)
                    .unwrap()
                    .1
                    .into()
            })
        });
        symbol_resolve::resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.contains("unknown binding")),
            "private visited API leaked: {name}"
        );
    }
}
