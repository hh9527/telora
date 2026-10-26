use super::*;
const SOURCE: &str = r#"
use std::build as build;
use std::value::{Value};
use std::codec as codec;
use std::type_desc as td;
type Count = struct(Int);
type Prepared = struct {apply: Fn(Int) -> Int, token: Type};
pub def answer: Fn(Int) -> Int = build::stage(
    fn() { {runtime: {apply: fn(x: Int) { x + 7 }, token: (Count).type}.ty!(Prepared), information: {"example": codec::encode(True)}} },
    fn(prepared) { fn(input) { if td::name(prepared.token) == td::name((Count).type) { prepared.apply(input) } else { fail!("type identity lost") } } }
);
"#;
#[test]
fn ordinary_function_build_restores_without_reexecuting_a() {
    let mir = graph(SOURCE);
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
        .publish(100_000_000, 64 * 1024 * 1024)
        .unwrap();
    assert!(wasmparser::Parser::new(0).parse_all(&bytes).any(|part| matches!(part, Ok(wasmparser::Payload::CustomSection(section)) if section.name() == "telora.build.information")));
    for _ in 0..2 {
        let mut session = crate::staged_build::load(&bytes, 100_000_000, 64 * 1024 * 1024).unwrap();
        assert_eq!(
            session.call(&[serde_json::json!(5)]).unwrap(),
            serde_json::json!(12)
        );
        assert_eq!(
            session.call(&[serde_json::json!(8)]).unwrap(),
            serde_json::json!(15)
        );
    }
}
#[test]
fn ordinary_execution_rejects_build_stage() {
    let bytes = compile(SOURCE).unwrap();
    let mut session = crate::session::Session::load(&bytes, 100_000_000).unwrap();
    let result = session
        .initialize()
        .and_then(|_| session.call(&[serde_json::json!(1)]).map(|_| ()));
    assert!(
        result.is_err(),
        "stage must not run outside the build scope"
    );
}
