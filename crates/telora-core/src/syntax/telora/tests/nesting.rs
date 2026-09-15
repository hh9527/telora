use super::*;

#[test]
fn deeply_nested_input_is_rejected_before_recursive_parsing() {
    // Exercise the real public parser on a Windows-sized stack, including the
    // accepted boundary. Generated input is intentional for depth regressions.
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            for depth in [32, 33, 180, 2000] {
                let source = format!("{}0{}", "fn(x) { ".repeat(depth), " }".repeat(depth));
                let mut sources = crate::source::SourceDatabase::default();
                let id = sources.add("nested.telora", &source);
                let parsed = parse(id, &source);
                let document = crate::document::DocumentText::new(&source);
                let from_document = parse_document(id, &document);
                assert_eq!(parsed.has_errors(), from_document.has_errors());
                if depth == 32 {
                    assert!(!parsed.has_errors(), "{:?}", parsed.diagnostics);
                } else {
                    assert!(
                        parsed
                            .diagnostics
                            .iter()
                            .any(|d| d.message.contains("syntax nesting exceeds"))
                    );
                    assert!(!parsed.diagnostics[0].labels.is_empty());
                }
            }
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn nesting_uses_tokens_and_mismatches_do_not_cancel_openers() {
    for (source, rejected) in [
        (format!("\"{}\"", "[".repeat(2000)), false),
        (format!("# {}\n0", "{".repeat(2000)), false),
        (format!("{}0{}", "[".repeat(33), "]".repeat(33)), true),
        (format!("{}0", "(]".repeat(33)), true),
        (format!("{}0{}", "`\\{".repeat(33), "}`".repeat(33)), true),
    ] {
        let mut diagnostics = Vec::new();
        let (tokens, spans) = lexer::tokenize(&source, &mut diagnostics);
        assert_eq!(
            super::super::nesting::check(&tokens, &spans).is_some(),
            rejected
        );
    }
}
