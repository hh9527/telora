use super::*;
use crate::{
    collection_alias,
    plan::{Key, Special},
    session::Session,
};

fn decisions(source: &str) -> Vec<&'static str> {
    let mir = graph(source);
    mir.hir
        .iter()
        .flat_map(|h| h.children.iter().map(|e| e.node))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|node| {
            matches!(
                mir.hir[node.index()].kind,
                telora_core::mir::HirKind::Closure
            )
        })
        .flat_map(|node| {
            collection_alias::analyze(
                &mir,
                Key {
                    node,
                    instance: None,
                    callable: true,
                    special: Special::Normal,
                },
            )
        })
        .map(|c| c.decision)
        .collect()
}
#[test]
fn alias_candidates_require_local_storage_and_account_for_aliases_and_captures() {
    for (body, expected) in [
        (
            "let a=[1]; let b=a; array::push(b,2)",
            "local_transfer_candidate",
        ),
        (
            "let a=[1]; let b=array::push(a,2); array::push(a,3)",
            "multiple_uses_or_escape",
        ),
        (
            "let a=[1]; let f=fn(){a}; array::push(a,2)",
            "multiple_uses_or_escape",
        ),
        (
            "let a=[1]; let box=(a,); array::push(a,2)",
            "multiple_uses_or_escape",
        ),
        ("array::push(input,2)", "parameter_capture_or_global"),
        (
            "let a=map::empty().ty!(Map(Int,Int)); map::keys(a)",
            "no_update",
        ),
    ] {
        let source = format!(
            "use std::array as array; use std::map as map; pub def answer:Fn(Array(Int))->Array(Int)=fn(input){{{body}}};"
        );
        let result = decisions(&source);
        if expected == "no_update" {
            assert!(result.is_empty());
        } else {
            assert!(result.contains(&expected), "{body}: {result:?}");
        }
    }
}
#[test]
fn alias_candidates_do_not_confuse_views_or_captured_parameters_with_owned_storage() {
    let cases = [
        "use std::array as a;use std::map as m;pub def answer:Fn()->Array(Int)=fn(){let map=m::from_pairs([(1,2)]);a::push(m::values(map),3)};",
        "use std::array as a;pub def answer:Fn(Array(Int))->Fn()->Array(Int)=fn(input){fn(){a::push(input,3)}};",
        "use std::string as s; pub def answer:Fn(String)->String=fn(input){s::join([input,\"tail\"],\"\")};",
    ];
    for source in cases {
        let result = decisions(source);
        assert!(!result.is_empty());
        assert!(!result.contains(&"local_transfer_candidate"), "{result:?}");
    }
}

#[test]
fn alias_candidates_cover_all_three_collection_backends() {
    for source in [
        r#"use std::map as m;pub def answer:Fn()->Map(Int,Int)=fn(){let a=m::from_pairs([(1,2)]);m::insert(a,2,3)};"#,
        r#"use std::array as a;pub def answer:Fn()->Array(Int)=fn(){let xs=[1,2];a::push(xs,3)};"#,
        r#"use std::string as s;pub def answer:Fn()->String=fn(){let text=s::join(["a","b"],"");s::join([text,"c"],"")};"#,
    ] {
        assert!(
            decisions(source).contains(&"local_transfer_candidate"),
            "{source}"
        );
    }
}

// Test-only extension inside the trusted std module exercises exactly ArrayMut,
// without adding a public builder API to the language or production library.
pub(super) fn benchmark_mir(source: &str) -> Mir {
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
        if name == "@src/main" {
            return Ok(source.into());
        }
        let mut text = static_sources::BUILTINS
            .iter()
            .find(|(n, _)| *n == name)
            .unwrap()
            .1
            .to_owned();
        if name == "std/pipeline" {
            text.push_str(
                &std::fs::read_to_string(format!(
                    "{}/tests/fixtures/collection-builder-probes.telora",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .unwrap(),
            );
        }
        Ok(text)
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    mir
}
pub(super) fn benchmark_artifact(source: &str) -> Vec<u8> {
    let mir = benchmark_mir(source);
    let symbol = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|s| mir.symbols[s.index()].name == "answer")
        .unwrap();
    crate::compile_executable(&mir.seal_export(symbol).unwrap()).unwrap()
}

#[test]
#[ignore = "controlled collection-builder comparison; run explicitly with --nocapture"]
fn measure_collection_aliasing_potential() {
    let mut rows = vec![];
    for (kind, ty, plain, bulk, summary) in [
        (
            "map_insert",
            "Map(Int,Int)",
            "a::fold(xs,m::empty(),fn(acc,x){m::insert(acc,x,x)})",
            "p::run(p::from_array(xs)|>p::collect_map(fn(x){x},fn(x){x}))",
            "(m::length(result),a::fold(m::values(result),0,fn(acc,x){acc+x}))",
        ),
        (
            "map_overwrite64",
            "Map(Int,Int)",
            "a::fold(xs,m::empty(),fn(acc,x){m::insert(acc,x%64,x)})",
            "p::run(p::from_array(xs)|>p::collect_map(fn(x){x%64},fn(x){x}))",
            "(m::length(result),a::fold(m::values(result),0,fn(acc,x){acc+x}))",
        ),
        (
            "array_push",
            "Array(Int)",
            "a::fold(xs,[].ty!(Array(Int)),fn(acc,x){a::push(acc,x)})",
            "p::alias_bench_array(xs)",
            "(a::length(result),a::fold(result,0,fn(acc,x){acc+x}))",
        ),
        (
            "string_append",
            "String",
            "a::fold(xs,\"\",fn(acc,x){s::join([acc,\"abcdefgh\"],\"\")})",
            "p::run(p::from_array(xs)|>p::map(fn(x){\"abcdefgh\"})|>p::collect_string())",
            "(s::length(result),if s::starts_with(result,\"abcdefgh\") {1}else{0})",
        ),
    ] {
        let mut expected = std::collections::BTreeMap::new();
        for (mode, expression) in [("immutable", plain), ("builder", bulk)] {
            let source = format!(
                "use std::array as a;use std::map as m;use std::pipeline as p;use std::string as s;pub def answer:Fn(Array(Int))->(Int,Int)=fn(xs){{let result:{ty}={expression};{summary}}};"
            );
            let artifact = benchmark_artifact(&source);
            for count in [128, 512, 2048] {
                let mut samples = vec![];
                for _ in 0..3 {
                    let mut session =
                        Session::load_with_limits(&artifact, 2_000_000_000, 256 * 1024 * 1024)
                            .unwrap();
                    session
                        .configure_gc(crate::gc::GcConfig {
                            allocation_interval: 0,
                            heap_limit: 256 * 1024 * 1024,
                        })
                        .unwrap();
                    session.initialize().unwrap();
                    let signature = session.manifest.types[session.manifest.entry_type as usize]
                        .arguments
                        .clone();
                    let input = session
                        .input_value(
                            signature[0],
                            &serde_json::json!((0..count).collect::<Vec<_>>()),
                        )
                        .unwrap();
                    let entry = crate::transport::Value {
                        pointer: session.entry().unwrap(),
                        ty: session.manifest.entry_type,
                    };
                    let allocated = session.gc_stats().unwrap().allocated_total;
                    let fuel = session.store.get_fuel().unwrap();
                    let start = std::time::Instant::now();
                    let output = session.invoke_values(entry, &[input]).unwrap();
                    let seconds = start.elapsed().as_secs_f64();
                    let fuel = fuel - session.store.get_fuel().unwrap();
                    let allocated = session.gc_stats().unwrap().allocated_total - allocated;
                    let value = session.output_value(output).unwrap();
                    let peak = session.gc_stats().unwrap();
                    let pin = session.pin_value(output).unwrap();
                    session.collect_garbage().unwrap();
                    let retained = session.gc_stats().unwrap().managed_bytes;
                    let relocated = session.pinned_value(pin).unwrap();
                    assert_eq!(session.output_value(relocated).unwrap(), value);
                    if mode == "immutable" {
                        expected.insert(count, value.clone());
                    } else {
                        assert_eq!(value, expected[&count]);
                    }
                    samples.push(serde_json::json!({"seconds":seconds,"fuel":fuel,"allocated_bytes":allocated,"managed_before_gc":peak.managed_bytes,"linear_memory_bytes":peak.memory_bytes,"managed_after_gc":retained}));
                }
                let row = serde_json::json!({"kind":kind,"mode":mode,"count":count,"artifact_bytes":artifact.len(),"samples":samples});
                println!("{row}");
                rows.push(row);
            }
        }
    }
    if let Some(path) = std::env::var_os("TELORA_ALIAS_BENCH_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    }
}
