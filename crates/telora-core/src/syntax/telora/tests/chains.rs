use super::*;

#[test]
fn long_chains_parse_and_lower_on_a_small_stack() {
    const PROBE: &str = "TELORA_PARSER_CHAIN_PROBE";
    if std::env::var_os(PROBE).is_none() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "syntax::telora::tests::chains::long_chains_parse_and_lower_on_a_small_stack",
                "--nocapture",
            ])
            .env(PROBE, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            for (source, valid) in [
                ("do { 1;; }".into(), false),
                ("do { 1; let a = 2; ; }".into(), false),
                (format!("do {{ {}0 }}", "1; ".repeat(5000)), true),
                (format!("{}1", "-".repeat(10000)), true),
                (format!("{}Bool", "Fn(Int) -> ".repeat(3000)), true),
                (
                    format!(
                        "{}{{ 0 }}",
                        "if True { 1 } else if let x = 1 { x } else ".repeat(1000)
                    ),
                    true,
                ),
                ("!".repeat(10000), false),
                ("Fn(Int) -> ".repeat(3000), false),
                ("if True { 1 } else ".repeat(2000), false),
            ] {
                let mut mir = crate::mir::Mir::default();
                let id = mir.sources.add("chain.telora", &source);
                let parsed = parse_document(id, mir.sources.get(id).text());
                assert_eq!(!parsed.has_errors(), valid, "{:?}", parsed.diagnostics);
                // Iterative traversal checks losslessness without making this test
                // itself depend on the native stack depth of the resulting CST.
                let mut pending = vec![NodeRef::ROOT];
                let mut reconstructed = String::new();
                while let Some(node) = pending.pop() {
                    match parsed.syntax.get(node) {
                        Node::Token(..) => {
                            reconstructed.push_str(&source[parsed.syntax.span(node)])
                        }
                        Node::Rule(..) => {
                            let children = parsed.syntax.children(node).collect::<Vec<_>>();
                            pending.extend(children.into_iter().rev());
                        }
                    }
                }
                assert_eq!(reconstructed, source);
                if valid {
                    let lowered = crate::hir_lower::lower_module(
                        &mut mir,
                        crate::mir::ModuleId(0),
                        id,
                        &parsed.syntax,
                    );
                    assert!(lowered.diagnostics.is_empty(), "{:?}", lowered.diagnostics);
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
