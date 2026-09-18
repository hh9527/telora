use super::*;

#[test]
fn expression_chains_reuse_consumed_locals_without_losing_live_values() {
    for (fixture, export, expected) in [
        ("expression-call-chain.telora", "answer", 1_619_100),
        ("expression-call-chain.telora", "single", 0),
        ("expression-sum-chain.telora", "answer", 12_497_500),
    ] {
        let source = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(fixture),
        )
        .unwrap();
        let bytes = compile_export(&source, export).unwrap();
        let maximum = wasmparser::Parser::new(0)
            .parse_all(&bytes)
            .filter_map(|payload| match payload.unwrap() {
                wasmparser::Payload::CodeSectionEntry(body) => Some(
                    body.get_locals_reader()
                        .unwrap()
                        .into_iter()
                        .map(|local| local.unwrap().0)
                        .sum::<u32>(),
                ),
                _ => None,
            })
            .max()
            .unwrap();
        assert!(maximum < 1000, "{fixture}:{export}: {maximum} locals");
        // Initialization retains and traces 1,800 distinct captured closures.
        // This test exercises codegen locals, not the fuel boundary.
        let mut session = crate::session::Session::load(&bytes, 1_000_000_000).unwrap();
        session.initialize().unwrap();
        for _ in 0..2 {
            assert_eq!(session.call(&[]).unwrap(), serde_json::json!(expected));
            assert!(session.diagnostics().unwrap().is_empty());
            session.collect_work(&[]).unwrap();
        }
    }
}
