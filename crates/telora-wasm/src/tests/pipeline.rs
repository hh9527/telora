use super::*;

const SHARING: &str = r#"
use std::pipeline as p;
def shared: p::Node(Int) = p::merge(p::from_array([1,2,1]), p::from_array([3,2]))
    |> p::distinct_by(fn(n) { `key\{n}` }) |> p::map(fn(n) { dbg!(n, "shared") });
def cached: p::Node(Int) = shared |> p::cache();
pub def answer: Fn(Int) -> (Array(Int), Dict(Int)) = fn(mode) {
    let unused = p::from_array([0]) |> p::map(fn(n) { fail!("eager pipeline") });
    let c = if mode == 0 { shared } else { cached };
    let f = c |> p::filter(fn(n) { n > 1 }) |> p::map(fn(n) { n * 10 }) |> p::collect();
    let g = c |> p::filter_map(fn(n) { if n == 2 { None } else { Some(n) } })
        |> p::collect_dict(fn(n) { `key\{n}` }, fn(n) { n });
    p::run(p::both(f, g))
};
"#;

#[test]
fn pipeline_is_lazy_shares_nodes_per_run_and_caches_only_when_requested() {
    let bytes = compile(SHARING).unwrap();
    let mut s = crate::session::Session::load(&bytes, 200_000_000).unwrap();
    s.set_debug_enabled(true).unwrap();
    s.initialize().unwrap();
    assert!(s.take_debug_events().unwrap().is_empty());
    let expected = serde_json::json!([[20,30], {"key1":1,"key3":3}]);
    for (mode, count) in [(0, 3), (0, 3), (1, 3), (1, 0), (0, 3), (1, 0)] {
        assert_eq!(s.call(&[mode.into()]).unwrap(), expected);
        let events = s.take_debug_events().unwrap();
        // Shared node is visited once for both terminals; cache has its own
        // slot and remains valid even when the uncached source is run again.
        assert_eq!(events.len(), count);
    }
}

#[test]
fn pipeline_explicit_cache_survives_gc_and_independent_source_runs() {
    let bytes = compile(SHARING).unwrap();
    let mut s = crate::session::Session::load(&bytes, 500_000_000).unwrap();
    s.configure_gc(crate::gc::GcConfig {
        allocation_interval: 1,
        heap_limit: 32 * 1024 * 1024,
    })
    .unwrap();
    s.set_debug_enabled(true).unwrap();
    s.initialize().unwrap();
    for (mode, count) in [(1, 3), (1, 0), (0, 3), (1, 0), (0, 3)] {
        let value = s.call(&[mode.into()]).unwrap();
        assert_eq!(value[0], serde_json::json!([20, 30]));
        assert_eq!(s.take_debug_events().unwrap().len(), count);
        s.collect_garbage().unwrap();
    }
}

#[test]
fn pipeline_private_buffers_preserve_order_unicode_and_input_aliases() {
    let source = r#"
use std::pipeline as p;
use std::array as array;
pub def answer: Fn(Array(String)) -> (Array(String), (Array(String), (Dict(String), String))) = fn(input) {
    let source = p::from_array(input);
    let unique = source |> p::distinct_by(fn(s) { s });
    let dict = source |> p::collect_dict(fn(s) { "last" }, fn(s) { s });
    let text = source |> p::collect_string();
    (input, p::run(p::both(unique |> p::collect(), p::both(dict, text))))
};
"#;
    let bytes = compile(source).unwrap();
    let mut s = crate::session::Session::load(&bytes, 10_000_000_000).unwrap();
    s.configure_gc(crate::gc::GcConfig {
        allocation_interval: 1,
        heap_limit: 32 * 1024 * 1024,
    })
    .unwrap();
    s.initialize().unwrap();
    let mut input: Vec<String> = (0..24).map(|n| format!("é🦀{n}")).collect();
    input.extend(["é🦀1".into(), "尾".into(), "".into()]);
    for _ in 0..2 {
        let result = s.call(&[serde_json::json!(input)]).unwrap();
        assert_eq!(result[0], serde_json::json!(input));
        assert_eq!(result[1][0].as_array().unwrap().len(), 26);
        assert_eq!(result[1][0][23], "é🦀23");
        assert_eq!(result[1][1][0], serde_json::json!({"last":""}));
        assert_eq!(result[1][1][1], input.concat());
        s.collect_garbage().unwrap();
    }
    assert_eq!(
        s.call(&[serde_json::json!([])]).unwrap(),
        serde_json::json!([[], [[], [{}, ""]]])
    );
}

#[test]
fn pipeline_failures_clear_pending_and_do_not_publish_cache() {
    let source = r#"
use std::pipeline as p;
use std::diagnostics as diagnostics;
def bad: p::Output(Array(Int)) = p::from_array([1]) |> p::map(fn(n) { fail!("calculation failed") }.ty!(Fn(Int) -> Int)) |> p::cache() |> p::collect();
def attempt: Fn() -> String = fn() {
    match diagnostics::with_diagnostics(fn(value: ()) { p::run(bad) })(()) {
        Ok(_) => "unexpected", Err(reports) => reports[0].message,
    }
};
pub def answer: Fn() -> (String, String, Array(Int)) = fn() {
    (attempt(), attempt(), p::run(p::from_array([7]) |> p::collect()))
};
"#;
    let bytes = compile(source).unwrap();
    let mut s = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    s.initialize().unwrap();
    for _ in 0..2 {
        assert_eq!(
            s.call(&[]).unwrap(),
            serde_json::json!(["calculation failed", "calculation failed", [7]])
        );
    }
}

#[test]
fn pipeline_builders_and_cache_slots_cannot_be_imported() {
    for name in [
        "create_mut_builder",
        "extend",
        "finish",
        "new_run",
        "new_slots",
        "memo",
        "DictMut",
        "ArrayMut",
        "StringMut",
        "Run",
        "Slot",
    ] {
        let source = format!("use std::pipeline::{{{name}}}; pub def answer = {name};");
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
        let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, module| {
            Ok(if module == "@src/main" {
                source.clone()
            } else {
                static_sources::BUILTINS
                    .iter()
                    .find(|(name, _)| *name == module)
                    .unwrap()
                    .1
                    .into()
            })
        });
        symbol_resolve::resolve(&mut mir);
        assert!(
            mir.diagnostics
                .iter()
                .any(|d| d.message.contains(name) && d.message.contains("unknown")),
            "{name}: {:?}",
            mir.diagnostics
        );
    }
}

#[test]
fn pipeline_recursive_nodes_report_cycles_and_allow_retry() {
    let source = r#"
use std::pipeline as p;
use std::diagnostics as diagnostics;
def cyclic: p::Node(Int) = p::from_array([1]) |> p::map(fn(n) { p::run(cyclic |> p::collect())[0] });
pub def answer: Fn() -> String = fn() {
    match diagnostics::with_diagnostics(fn(value: ()) { p::run(cyclic |> p::collect()) })(()) {
        Ok(_) => "unexpected", Err(reports) => reports[0].message,
    }
};
"#;
    let bytes = compile(source).unwrap();
    let mut s = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    s.initialize().unwrap();
    for _ in 0..2 {
        assert_eq!(
            s.call(&[]).unwrap(),
            "pipeline node computation depends on itself"
        );
    }
}

#[test]
fn pipeline_warmed_cache_preserves_run_identity_across_build_time_relocation() {
    let source = r#"
use std::pipeline as p;
use std::build as build;
pub def answer: Fn() -> Array(Int) = build::stage(fn() {
    let c = p::from_array([4,5]) |> p::map(fn(n) { dbg!(n) }) |> p::cache();
    let warm = p::run(c |> p::collect());
    if warm != [4,5] { fail!("warmup failed") } else { () };
    {runtime: c, information: {}}
}, fn(c) { fn() { p::run(c |> p::collect()) } });
"#;
    let mir = graph(source);
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
        .publish(200_000_000, 64 * 1024 * 1024)
        .unwrap();
    for _ in 0..2 {
        let mut s = crate::staged_build::load(&bytes, 200_000_000, 64 * 1024 * 1024).unwrap();
        s.set_debug_enabled(true).unwrap();
        for _ in 0..2 {
            assert_eq!(s.call(&[]).unwrap(), serde_json::json!([4, 5]));
            assert!(s.take_debug_events().unwrap().is_empty());
            s.collect_garbage().unwrap();
        }
    }
}
