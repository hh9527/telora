use super::*;

#[test]
fn primitive_text_fusion_keeps_display_values_sharing_and_debug_order() {
    let source = r#"
use std::fmt as fmt;
type Custom = struct { value: Int };
impl fmt::Display for Custom { fmt: fn(value, f) {
    fmt::write_str(fmt::write_int(fmt::write_str(f, "custom("), dbg!(value.value)), ")")
} };
pub def answer: Fn(Int, Float, String) -> (Array(String), Bool, Bool) = fn(n, f, s) {
    let shared=fmt::from_int(n);
    let a=`n=\{dbg!(n)};f=\{f};s=\{s}`;
    let b=fmt::render(fmt::from_int(n));
    let c=fmt::render(fmt::from_float(f));
    let d=fmt::render(fmt::from_string(s));
    let e=`\{n}/\{{value:n}.ty!(Custom)}/\{s}`;
    ([a,b,c,d,e,fmt::render(shared),fmt::render(shared)],
     shared==fmt::from_int(n), shared==fmt::from_string(b))
};"#;
    let bytes = compile(source).unwrap();
    let mut session = crate::session::Session::load(&bytes, 1_000_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 32 * 1024 * 1024,
        })
        .unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        let result = session
            .call(&[7.into(), serde_json::json!(-0.0), "é🦀\n".into()])
            .unwrap();
        assert_eq!(
            result,
            serde_json::json!([
                [
                    "n=7;f=-0;s=é🦀\n",
                    "7",
                    "-0",
                    "é🦀\n",
                    "7/custom(7)/é🦀\n",
                    "7",
                    "7"
                ],
                true,
                false
            ])
        );
        assert_eq!(
            session
                .take_debug_events()
                .unwrap()
                .iter()
                .map(|event| event.repr.clone())
                .collect::<Vec<_>>(),
            ["7", "7"]
        );
        session.collect_garbage().unwrap();
    }
}

#[test]
fn primitive_text_fusion_preserves_first_failure() {
    for expression in [
        r#"`\{bad()}/\{dbg!(2)}`"#,
        r#"fmt::render(fmt::write_str(fmt::write_int(fmt::empty(), bad()), dbg!("later")))"#,
    ] {
        let source = format!(
            "use std::fmt as fmt; def bad: Fn() -> Int = fn() {{ fail!(\"first\") }}; pub def answer: Fn() -> String = fn() {{ {expression} }};"
        );
        let bytes = compile(&source).unwrap();
        let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
        session.set_debug_enabled(true).unwrap();
        session.initialize().unwrap();
        assert!(session.call(&[]).unwrap_err().contains("first"));
        assert!(session.take_debug_events().unwrap().is_empty());
    }
}

#[test]
fn primitive_text_fusion_keeps_append_chain_evaluation_and_json_escaping() {
    let bytes = compile(r#"
use std::fmt as fmt;
pub def answer: Fn(Int, String) -> Array(String) = fn(n, s) {
    [fmt::render(fmt::write_str(fmt::write_int(fmt::write_str(fmt::empty(), dbg!(s)), dbg!(n)), dbg!("!"))),
     fmt::render(fmt::encode_json_string(fmt::write_str(fmt::empty(), "json="), s))]
};"#).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    session
        .configure_gc(crate::gc::GcConfig {
            allocation_interval: 1,
            heap_limit: 32 * 1024 * 1024,
        })
        .unwrap();
    session.set_debug_enabled(true).unwrap();
    session.initialize().unwrap();
    for _ in 0..2 {
        assert_eq!(
            session.call(&[7.into(), "é\"\n".into()]).unwrap(),
            serde_json::json!(["é\"\n7!", "json=\"é\\\"\\n\""])
        );
        let events = session.take_debug_events().unwrap();
        assert_eq!(
            events.iter().map(|e| e.repr.as_str()).collect::<Vec<_>>(),
            ["\"é\\\"\\n\"", "7", "\"!\""]
        );
        session.collect_garbage().unwrap();
    }
}
