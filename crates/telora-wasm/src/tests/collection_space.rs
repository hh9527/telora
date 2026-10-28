//! Controlled variants, not a production rewrite or an alias-safety proof.
use super::collection_alias::benchmark_artifact;
use crate::{gc::GcConfig, session::Session, transport::Value};
use serde_json::Value as Json;
use std::collections::BTreeMap;

const IMPORTS: &str = "use std::array as a;use std::map as m;use std::pipeline as p;use std::string as s;use std::seq as q;use std::sink as sink;";
const EAGER: &str = "a::map(a::filter(a::map(xs,fn(x){x+1}),fn(x){x%3!=0}),fn(x){x*2})";
const PIPE: &str =
    "p::from_array(xs)|>p::map(fn(x){x+1})|>p::filter(fn(x){x%3!=0})|>p::map(fn(x){x*2})";
const FUSED: &str =
    "q::map(q::filter(q::map(q::from_array(xs),fn(x){x+1}),fn(x){x%3!=0}),fn(x){x*2})";

struct Case {
    name: &'static str,
    ty: &'static str,
    variants: Vec<(&'static str, String)>,
}
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "array_chain",
            ty: "Array(Int)",
            variants: vec![
                ("eager", EAGER.into()),
                ("pipeline", format!("p::run(({PIPE})|>p::collect())")),
                ("owned_stage_order", "p::alias_owned_chain(xs)".into()),
                ("owned_noescape_oracle", "p::alias_owned_chain_noescape(xs)".into()),
                ("fused", format!("q::collect({FUSED})")),
            ],
        },
        Case {
            name: "interprocedural_array_chain",
            ty: "Array(Int)",
            variants: vec![
                ("conservative_wrapped", "p::alias_owned_wrapped_chain(xs)".into()),
                ("summary_selected", "p::alias_owned_checked_chain(xs)".into()),
            ],
        },
        Case {
            name: "captured_record_array_chain",
            ty: "Array(Int)",
            variants: vec![
                ("conservative_captured", "p::alias_owned_captured_conservative(xs,1)".into()),
                ("summary_captured", "p::alias_owned_captured_checked(xs,1)".into()),
            ],
        },
        Case {
            name: "map_chain",
            ty: "Map(Int,Int)",
            variants: vec![
                (
                    "immutable",
                    format!("a::fold({EAGER},m::empty(),fn(acc,x){{m::insert(acc,x,x)}})"),
                ),
                (
                    "pipeline",
                    format!("p::run(({PIPE})|>p::collect_map(fn(x){{x}},fn(x){{x}}))"),
                ),
                (
                    "fused_builder",
                    format!("p::alias_seq_map({FUSED},fn(x){{x}})"),
                ),
            ],
        },
        Case {
            name: "string_chain",
            ty: "String",
            variants: vec![
                (
                    "immutable",
                    format!(
                        "a::fold({EAGER},\"\",fn(acc,x){{s::join([acc,if x%2==0 {{\"abcdefgh\"}}else{{\"ABCDEFGH\"}}],\"\")}})"
                    ),
                ),
                (
                    "pipeline",
                    format!(
                        "p::run(({PIPE})|>p::map(fn(x){{if x%2==0 {{\"abcdefgh\"}}else{{\"ABCDEFGH\"}}}})|>p::collect_string())"
                    ),
                ),
                ("fused_builder", format!("p::alias_seq_string({FUSED})")),
            ],
        },
        Case {
            name: "shared_fork",
            ty: "(Array(Int),Array(Int))",
            variants: vec![
                (
                    "eager",
                    format!(
                        "do{{let c={EAGER};(a::filter(c,fn(x){{x%4==0}}),a::map(c,fn(x){{x+7}}))}}"
                    ),
                ),
                (
                    "pipeline",
                    format!(
                        "do{{let c={PIPE};p::run(p::both(c|>p::filter(fn(x){{x%4==0}})|>p::collect(),c|>p::map(fn(x){{x+7}})|>p::collect()))}}"
                    ),
                ),
                (
                    "fused_shared_snapshot",
                    format!(
                        "do{{let c=q::collect({FUSED});(q::collect(q::filter(q::from_array(c),fn(x){{x%4==0}})),q::collect(q::map(q::from_array(c),fn(x){{x+7}})))}}"
                    ),
                ),
                (
                    "fused_fanout_pure",
                    format!(
                        "q::compute({FUSED},sink::both(sink::filter(fn(x){{x%4==0}},sink::collect()),sink::map(fn(x){{x+7}},sink::collect())))"
                    ),
                ),
            ],
        },
        Case {
            name: "mixed_three_collectors",
            ty: "(Map(Int,Int),Array(Int),String)",
            variants: vec![
                (
                    "pipeline",
                    format!(
                        "do{{let c={PIPE};let r=p::run(p::both(c|>p::collect_map(fn(x){{x%64}},fn(x){{x}}),p::both(c|>p::collect(),c|>p::map(fn(x){{if x%2==0 {{\"abcdefgh\"}}else{{\"ABCDEFGH\"}}}})|>p::collect_string())));(r.0,r.1.0,r.1.1)}}"
                    ),
                ),
                ("fused_builder", format!("p::alias_seq_mixed({FUSED})")),
            ],
        },
        Case {
            name: "distinct64",
            ty: "Array(Int)",
            variants: vec![
                (
                    "pipeline",
                    format!(
                        "p::run(({PIPE})|>p::distinct_by(fn(x){{`key\\{{x%64}}`}})|>p::collect())"
                    ),
                ),
                ("fused_builder", format!("p::alias_seq_distinct({FUSED})")),
            ],
        },
        Case {
            name:"map_shared_input",ty:"(Map(Int,Int),Map(Int,Int),Array(Int),Array(Int))",variants:vec![
                ("immutable","do{let initial=m::from_pairs(a::map(xs,fn(x){(x,x)}));let updated=a::fold(xs,initial,fn(acc,x){m::insert(acc,x,x*10)});(initial,updated,m::keys(initial),m::values(initial))}".into()),
                ("copy_once_builder","do{let initial=m::from_pairs(a::map(xs,fn(x){(x,x)}));let updated=p::alias_update_initial(initial,xs);(initial,updated,m::keys(initial),m::values(initial))}".into()),
            ]
        },
        Case {
            name:"array_shared_input",ty:"(Array(Int),Array(Int))",variants:vec![
                ("immutable","(xs,a::fold(xs,xs,fn(acc,x){a::push(acc,x)}))".into()),
                ("copy_once_builder","(xs,p::alias_append_initial(xs,xs))".into()),
            ]
        },
        Case {
            name:"string_shared_input",ty:"(String,String)",variants:vec![
                ("immutable",r#"do{let initial=s::join(a::map(xs,fn(x){"abcdefgh"}),"");(initial,a::fold(xs,initial,fn(acc,x){s::join([acc,"abcdefgh"],"")}))}"#.into()),
                ("borrow_chunks_builder",r#"do{let initial=s::join(a::map(xs,fn(x){"abcdefgh"}),"");(initial,p::alias_string_initial(initial,xs))}"#.into()),
            ]
        },
        Case {
            name:"map_missing_remove",ty:"Map(Int,Int)",variants:vec![
                ("immutable","do{let initial=m::from_pairs(a::map(xs,fn(x){(x,x)}));a::fold(xs,initial,fn(acc,x){m::remove(acc,-1)})}".into()),
                ("readonly_no_copy","do{let initial=m::from_pairs(a::map(xs,fn(x){(x,x)}));a::fold(xs,initial,fn(acc,x){if m::contains_key(acc,-1){m::remove(acc,-1)}else{acc}})}".into()),
            ]
        },
        Case {
            name:"callback_dispatch_count",ty:"Int",variants:vec![
                ("no_callback","q::count(q::from_array(xs))".into()),
                ("one_identity_callback","q::count(q::map(q::from_array(xs),fn(x){x}))".into()),
                ("three_identity_callbacks","q::count(q::map(q::map(q::map(q::from_array(xs),fn(x){x}),fn(x){x}),fn(x){x}))".into()),
            ]
        },
    ]
}
fn artifact(ty: &str, expression: &str) -> Vec<u8> {
    benchmark_artifact(&format!(
        "{IMPORTS}pub def answer:Fn(Array(Int))->{ty}=fn(xs){{{expression}}};"
    ))
}
fn session(bytes: &[u8], interval: u32) -> Session {
    let mut s = Session::load_with_limits(bytes, 2_000_000_000, 256 * 1024 * 1024).unwrap();
    s.configure_gc(GcConfig {
        allocation_interval: interval,
        heap_limit: 256 * 1024 * 1024,
    })
    .unwrap();
    s.initialize().unwrap();
    s
}
fn input(s: &mut Session, n: i32) -> (Value, Value) {
    let ty = s.manifest.entry_type;
    let arg = s.manifest.types[ty as usize].arguments[0];
    let input = s
        .input_value(arg, &serde_json::json!((0..n).collect::<Vec<_>>()))
        .unwrap();
    (
        Value {
            pointer: s.entry().unwrap(),
            ty,
        },
        input,
    )
}
#[test]
fn collection_space_variants_preserve_complete_values_and_gc_roots() {
    for case in cases() {
        let mut expected = BTreeMap::new();
        for (mode, expression) in &case.variants {
            let bytes = artifact(case.ty, expression);
            let mut s = session(&bytes, 1);
            for count in [0, 1, 2, 3, 17, 65] {
                let (entry, arg) = input(&mut s, count);
                let result = s.invoke_values(entry, &[arg]).unwrap();
                let value = s.output_value(result).unwrap();
                if let Some(prior) = expected.get(&count) {
                    assert_eq!(&value, prior, "{} {mode} n={count}", case.name);
                } else {
                    expected.insert(count, value.clone());
                }
                let pin = s.pin_value(result).unwrap();
                s.collect_garbage().unwrap();
                let relocated = s.pinned_value(pin).unwrap();
                assert_eq!(s.output_value(relocated).unwrap(), value);
                s.unpin_value(pin).unwrap();
            }
        }
    }
}
#[test]
fn destructive_array_probe_preserves_borrowed_input_and_filtermap_edges() {
    let source = r#"use std::pipeline as p;pub def answer:Fn(Array(Int))->(Array(Int),Array(Int),Array(Int))=fn(xs){
 (xs,p::alias_owned_chain(xs),p::alias_owned_filtermap(xs,fn(x){if x%2==0 {Some(x+10)}else{None}}))
 };"#;
    let bytes = benchmark_artifact(source);
    let mut s = session(&bytes, 1);
    for n in [0, 1, 2, 3, 64] {
        let result = s
            .call(&[serde_json::json!((0..n).collect::<Vec<_>>())])
            .unwrap();
        assert_eq!(
            result,
            serde_json::json!([
                (0..n).collect::<Vec<_>>(),
                (0..n)
                    .map(|x| x + 1)
                    .filter(|x| x % 3 != 0)
                    .map(|x| x * 2)
                    .collect::<Vec<_>>(),
                (0..n)
                    .filter(|x| x % 2 == 0)
                    .map(|x| x + 10)
                    .collect::<Vec<_>>()
            ])
        );
    }
}

#[test]
fn fused_and_stage_ordered_paths_have_distinct_observable_callback_order() {
    let note = r#"use std::diagnostics as d;def note:Fn(String,Int)->Int=fn(tag,x){let ignored:Option(Int)=warn!(tag);x};"#;
    let expressions = [
        (
            "pipeline",
            r#"p::run(p::from_array([1,2])|>p::map(fn(x){note("A",x)})|>p::map(fn(x){note("B",x)})|>p::collect())"#,
            vec!["A", "A", "B", "B"],
        ),
        (
            "owned",
            r#"p::alias_owned_steps([1,2],fn(x){note("A",x)},fn(x){True},fn(x){note("B",x)})"#,
            vec!["A", "A", "B", "B"],
        ),
        (
            "fused",
            r#"q::collect(q::map(q::map(q::from_array([1,2]),fn(x){note("A",x)}),fn(x){note("B",x)}))"#,
            vec!["A", "B", "A", "B"],
        ),
    ];
    for (name, expression, expected) in expressions {
        let source = format!(
            "{IMPORTS}{note}pub def answer:Fn()->Array(String)=fn(){{let r=d::with_diagnostics(fn(ignored:()){{{expression}}})(());match r{{Ok((v,reports))=>a::map(reports,fn(r){{r.message}}),Err(_)=>[\"unexpected\"]}}}};"
        );
        let mut s = session(&benchmark_artifact(&source), 1);
        assert_eq!(s.call(&[]).unwrap(), serde_json::json!(expected), "{name}");
    }
}

#[test]
#[ignore = "space survey: controlled stage-preserving and fused variants"]
fn measure_lazy_collection_space() {
    let mut rows = vec![];
    for case in cases() {
        if std::env::var("TELORA_COLLECTION_SPACE_CASES")
            .ok()
            .is_some_and(|names| !names.split(',').any(|n| n == case.name))
        {
            continue;
        }
        let mut expected: BTreeMap<i32, Json> = BTreeMap::new();
        for (mode, expression) in &case.variants {
            let bytes = artifact(case.ty, expression);
            for count in if matches!(case.name, "map_shared_input" | "map_missing_remove") {
                [128, 512, 1024]
            } else {
                [128, 512, 2048]
            } {
                let mut samples = vec![];
                for _ in 0..3 {
                    let mut s = session(&bytes, 0);
                    let (entry, arg) = input(&mut s, count);
                    let before = s.gc_stats().unwrap().allocated_total;
                    let fuel = s.store.get_fuel().unwrap();
                    let start = std::time::Instant::now();
                    let result = s.invoke_values(entry, &[arg]).unwrap();
                    let seconds = start.elapsed().as_secs_f64();
                    let fuel = fuel - s.store.get_fuel().unwrap();
                    let stats = s.gc_stats().unwrap();
                    let value = s.output_value(result).unwrap();
                    if let Some(prior) = expected.get(&count) {
                        assert_eq!(&value, prior, "{} {mode}", case.name);
                    } else {
                        expected.insert(count, value.clone());
                    }
                    let pin = s.pin_value(result).unwrap();
                    s.collect_garbage().unwrap();
                    let after = s.gc_stats().unwrap();
                    let relocated = s.pinned_value(pin).unwrap();
                    assert_eq!(s.output_value(relocated).unwrap(), value);
                    samples.push(serde_json::json!({"seconds":seconds,"fuel":fuel,"allocated_bytes":stats.allocated_total-before,"linear_memory_bytes":stats.memory_bytes,"managed_after_gc":after.managed_bytes}));
                }
                let row = serde_json::json!({"case":case.name,"mode":mode,"count":count,"artifact_bytes":bytes.len(),"samples":samples});
                println!("{row}");
                rows.push(row);
                if let Some(path) = std::env::var_os("TELORA_COLLECTION_SPACE_REPORT") {
                    std::fs::write(path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
                }
            }
        }
    }
}
