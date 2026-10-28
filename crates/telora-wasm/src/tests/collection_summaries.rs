use super::graph;
use crate::{
    collection_summaries::{Analysis, Summary},
    plan::{Plan, child},
};
use telora_core::mir::Role;
fn summary(source: &str, name: &str) -> Summary {
    let mir = graph(source);
    assert!(mir.diagnostics.is_empty(), "{:?}", mir.diagnostics);
    let answer = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|s| mir.symbols[s.index()].name == "answer")
        .unwrap();
    let executable = mir.seal_export(answer).unwrap();
    let plan = Plan::new(&executable).unwrap();
    let symbol = mir
        .symbols
        .iter()
        .find(|s| {
            s.name == name
                && s.module
                    .is_some_and(|m| mir.modules[m.index()].name == "@src/main")
        })
        .unwrap();
    let node = symbol
        .declarations
        .iter()
        .find_map(|d| child(&mir, *d, Role::Value).ok())
        .unwrap();
    let key = *plan
        .functions
        .keys()
        .find(|k| k.node == node && k.callable)
        .unwrap();
    Analysis::new(&mir, &plan).function(key)
}
#[test]
fn cross_function_scalar_summary_follows_wrappers_and_callable_arguments() {
    let s = summary(
        "def add:Fn(Int)->Int=fn(x){x+1};def twice:Fn(Int)->Int=fn(x){add(add(x))};def apply:Fn(Fn(Int)->Int,Int)->Int=fn(f,x){f(x)};pub def answer:Fn(Int)->Int=fn(x){apply(twice,x)};",
        "answer",
    );
    assert!(s.scalar_borrow_safe, "{s:?}");
    assert!(s.calls_followed >= 5, "{s:?}");
}
#[test]
fn cross_function_summaries_distinguish_borrowed_updated_and_fresh_results() {
    for (body, param, updated, fresh) in [
        ("identity(xs)", true, false, false),
        ("push_twice(xs)", true, true, false),
        ("new_array()", false, false, true),
    ] {
        let source = format!(
            "use std::array as a;def identity:Fn(Array(Int))->Array(Int)=fn(xs){{xs}};def push_once:Fn(Array(Int))->Array(Int)=fn(xs){{a::push(xs,1)}};def push_twice:Fn(Array(Int))->Array(Int)=fn(xs){{push_once(push_once(xs))}};def new_array:Fn()->Array(Int)=fn(){{[1,2]}};pub def answer:Fn(Array(Int))->Array(Int)=fn(xs){{{body}}};"
        );
        let s = summary(&source, "answer");
        assert!(!s.result.unknown, "{s:?}");
        assert_eq!(s.result.parameters.contains(&0), param, "{s:?}");
        assert_eq!(s.result.updated, updated, "{s:?}");
        assert_eq!(s.result.fresh, fresh, "{s:?}");
    }
}
#[test]
fn cross_function_scalar_borrow_proof_rejects_diagnostics_recursion_and_escaped_capture() {
    for helper in [
        "def helper:Fn(Int)->Int=fn(x){dbg!(x)};",
        "def helper:Fn(Int)->Int=fn(x){if x==0 {0}else{helper(x-1)}};",
        "def helper:Fn(Int)->Int=fn(x){let xs=[dbg!(x)];x+1};",
    ] {
        let s = summary(
            &format!("{helper} pub def answer:Fn(Int)->Int=fn(x){{helper(x)}};"),
            "answer",
        );
        assert!(!s.scalar_borrow_safe, "{s:?}");
    }
    let s = summary(
        "pub def answer:Fn(Int)->Fn()->Int=fn(x){fn(){x}};",
        "answer",
    );
    assert!(!s.scalar_borrow_safe);
    assert!(
        s.result.parameters.contains(&0),
        "captured closure must retain input provenance: {s:?}"
    );
}

#[test]
fn checked_array_lowering_selects_proven_wrappers_and_falls_back_for_forwarded_callbacks() {
    for (expression, expected) in [
        ("p::alias_owned_checked_chain(xs)", true),
        ("p::alias_owned_captured_checked(xs,1)", true),
        ("p::alias_owned_checked_dynamic(xs,fn(x){x+1})", false),
        ("p::alias_owned_checked_debug(xs)", false),
    ] {
        let source = format!(
            "use std::pipeline as p;pub def answer:Fn(Array(Int))->(Array(Int),Array(Int))=fn(xs){{(xs,{expression})}};"
        );
        let mir = super::collection_alias::benchmark_mir(&source);
        let answer = mir
            .exports
            .iter()
            .flatten()
            .copied()
            .find(|s| mir.symbols[s.index()].name == "answer")
            .unwrap();
        let executable = mir.seal_export(answer).unwrap();
        let plan = Plan::new(&executable).unwrap();
        assert_eq!(
            crate::collection_summaries::scalar_callbacks_safe(
                &mir,
                &plan,
                "alias_map_array_checked"
            ),
            expected,
            "{expression}"
        );
        let bytes = crate::compile_executable(&executable).unwrap();
        let mut session =
            crate::session::Session::load_with_limits(&bytes, 100_000_000, 256 * 1024 * 1024)
                .unwrap();
        session
            .configure_gc(crate::gc::GcConfig {
                allocation_interval: 1,
                heap_limit: 256 * 1024 * 1024,
            })
            .unwrap();
        session.set_debug_enabled(true).unwrap();
        session.initialize().unwrap();
        let result = session.call(&[serde_json::json!([0, 1, 2, 3])]).unwrap();
        assert_eq!(result[0], serde_json::json!([0, 1, 2, 3]));
        assert_eq!(
            result[1],
            if expected {
                serde_json::json!([2, 4, 8])
            } else if expression.contains("debug") {
                serde_json::json!([0, 1, 2, 3])
            } else {
                serde_json::json!([1, 2, 3, 4])
            }
        );
        assert_eq!(
            session.take_debug_events().unwrap().len(),
            if expression.contains("debug") { 4 } else { 0 }
        );
    }
}

#[test]
fn cross_function_fold_summary_traces_accumulator_through_helper() {
    let s = summary(
        "use std::array as a;use std::map as m;def update:Fn(Map(Int,Int),Int)->Map(Int,Int)=fn(acc,x){m::insert(acc,x,x)};def build:Fn(Array(Int))->Map(Int,Int)=fn(xs){a::fold(xs,m::empty(),fn(acc,x){update(acc,x)})};pub def answer:Fn(Array(Int))->Map(Int,Int)=fn(xs){build(xs)};",
        "answer",
    );
    assert!(!s.result.unknown, "{s:?}");
    assert!(s.result.fresh && s.result.updated, "{s:?}");
    assert!(
        s.updates.iter().any(|u| u.collection == "Map"
            && u.depth >= 4
            && !u.receiver.unknown
            && u.receiver.fresh),
        "{s:?}"
    );
}

#[test]
fn cross_function_summaries_preserve_map_views_and_string_input_dependencies() {
    let s = summary(
        "use std::map as m;def values:Fn(Map(Int,Int))->Array(Int)=fn(input){m::values(input)};pub def answer:Fn(Map(Int,Int))->Array(Int)=fn(input){values(input)};",
        "answer",
    );
    assert!(
        s.result.borrowed_view && s.result.parameters.contains(&0),
        "{s:?}"
    );
    let s = summary(
        "use std::string as s;def append:Fn(String)->String=fn(input){s::join([input,\"x\"],\"\")};pub def answer:Fn(String)->String=fn(input){append(input)};",
        "answer",
    );
    assert!(s.result.parameters.contains(&0), "{s:?}");
}

#[test]
fn field_sensitive_summary_separates_record_members_and_symbolic_paths() {
    let s = summary(
        "type Box=struct {left:Array(Int),right:Array(Int)};def pick:Fn(Box)->Array(Int)=fn(box){box.left};pub def answer:Fn(Array(Int),Array(Int))->Array(Int)=fn(xs,ys){pick({left:xs,right:ys}.ty!(Box))};",
        "answer",
    );
    assert_eq!(s.result.parameters, std::collections::BTreeSet::from([0]));
    assert!(!s.result.unknown, "{s:?}");
    let s = summary(
        "type Box=struct {left:Array(Int),right:Array(Int)};def pass:Fn(Array(Int))->Array(Int)=fn(xs){xs};pub def answer:Fn(Box)->Array(Int)=fn(box){pass(box.right)};",
        "answer",
    );
    assert!(!s.result.unknown && s.result.borrowed_view, "{s:?}");
    assert!(
        s.result
            .paths
            .iter()
            .any(|p| p.root == "parameter:0" && p.fields == ["right"]),
        "{s:?}"
    );
}
#[test]
fn captured_closure_values_keep_distinct_environments_and_join_safely() {
    let prefix = "def make:Fn(Int)->Fn()->Int=fn(x){fn(){x}};";
    let s = summary(
        &format!(
            "{prefix}pub def answer:Fn(Int,Int)->(Int,Int)=fn(a,b){{let left=make(a);let right=make(b);(left(),right())}};"
        ),
        "answer",
    );
    assert_eq!(
        s.result.members["0"].parameters,
        std::collections::BTreeSet::from([0]),
        "{s:?}"
    );
    assert_eq!(
        s.result.members["1"].parameters,
        std::collections::BTreeSet::from([1]),
        "{s:?}"
    );
    let s = summary(
        &format!(
            "{prefix}pub def answer:Fn(Int,Int,Bool)->Int=fn(a,b,choose){{let f=if choose {{make(a)}}else{{make(b)}};f()}};"
        ),
        "answer",
    );
    assert_eq!(
        s.result.parameters,
        std::collections::BTreeSet::from([0, 1]),
        "{s:?}"
    );
    assert!(!s.result.unknown, "{s:?}");
}
#[test]
fn closure_capture_returning_collection_retains_input_alias() {
    let s = summary(
        "def make:Fn(Array(Int))->Fn()->Array(Int)=fn(xs){fn(){xs}};def run:Fn(Fn()->Array(Int))->Array(Int)=fn(f){f()};pub def answer:Fn(Array(Int))->Array(Int)=fn(xs){run(make(xs))};",
        "answer",
    );
    assert!(!s.result.unknown, "{s:?}");
    assert_eq!(
        s.result.parameters,
        std::collections::BTreeSet::from([0]),
        "{s:?}"
    );
    assert!(!s.result.fresh);
}
#[test]
fn record_projection_does_not_erase_other_field_evaluation_effects() {
    let s = summary(
        "type Box=struct {left:Int,right:Int};pub def answer:Fn(Int,Int)->Int=fn(a,b){let box={left:a,right:dbg!(b)}.ty!(Box);box.left};",
        "answer",
    );
    assert!(s.opaque_or_effectful && !s.scalar_borrow_safe, "{s:?}");
    assert_eq!(
        s.result.parameters,
        std::collections::BTreeSet::from([0]),
        "{s:?}"
    );
}

#[test]
fn fold_fixed_point_covers_aliases_that_appear_after_multiple_iterations() {
    let s = summary(
        "use std::array as a;pub def answer:Fn(Array(Int),Array(Int),Array(Int),Array(Int))->Array(Int)=fn(items,x,y,z){let state=a::fold(items,(x,y,z),fn(acc,item){(acc.1,acc.2,acc.0)});state.0};",
        "answer",
    );
    assert!(!s.result.unknown, "{s:?}");
    assert_eq!(
        s.result.parameters,
        std::collections::BTreeSet::from([1, 2, 3]),
        "{s:?}"
    );
    assert!(s.fold_iterations >= 3 && s.fold_widenings == 0, "{s:?}");
}
#[test]
fn array_callback_contract_preserves_record_element_paths_and_effects() {
    let s = summary(
        "use std::array as a;type Row=struct {left:Int,right:Int};pub def answer:Fn(Array(Row))->Array(Int)=fn(rows){a::map(rows,fn(row){row.right})};",
        "answer",
    );
    let element = &s.result.members["*"];
    assert!(!element.unknown, "{s:?}");
    assert!(
        element.paths.iter().any(|p| p.fields == ["[*]", "right"]),
        "{s:?}"
    );
    let s = summary(
        "use std::array as a;pub def answer:Fn(Array(Int))->Array(Int)=fn(xs){a::map(xs,fn(x){dbg!(x)})};",
        "answer",
    );
    assert!(s.opaque_or_effectful);
}

#[test]
fn callback_site_report_distinguishes_filter_predicates_from_read_only_helpers() {
    let mir = graph(
        "use std::array as a;type Row=struct {n:Int};pub def answer:Fn(Array(Row))->Array(Row)=fn(rows){let ignored=a::any(rows,fn(row){row.n==0});a::filter(rows,fn(row){row.n>0})};",
    );
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|s| mir.symbols[s.index()].name == "answer")
        .unwrap();
    let executable = mir.seal_export(export).unwrap();
    let plan = Plan::new(&executable).unwrap();
    let summaries = crate::collection_summaries::report(&mir, &plan);
    let sites = crate::collection_summaries::callback_sites::report(&mir, &plan, &summaries);
    for op in ["filter", "any"] {
        assert!(
            sites.iter().any(|s| s["operation"] == op
                && s["scalar_result_borrow_safe"] == true
                && s["callback_input"] == "aggregate_or_other"),
            "{sites:?}"
        );
    }
}

#[test]
fn implicit_construction_checks_block_predicate_snapshot_elision() {
    let s = summary(
        "use std::blame::{BlameError};@check(fn(value){if value.n>0{Ok(())}else{Err(blame!(\"positive\",value.n))}})type Checked=struct {n:Int};pub def answer:Fn(Int)->Bool=fn(x){let checked:Checked={n:x};checked.n>0};",
        "answer",
    );
    assert!(!s.scalar_borrow_safe && s.opaque_or_effectful, "{s:?}");
    assert!(
        s.blockers.get("construction_check").copied().unwrap_or(0) > 0,
        "{s:?}"
    );
}
