#[test]
fn delimiter_recovery_preserves_following_declarations_and_missing_position() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/hir-lower/delimiter-recovery.telora");
    let text = std::fs::read_to_string(path).unwrap();
    let mut sources = crate::source::SourceDatabase::default();
    let source = sources.add("recovery.telora", &text);
    let parsed = super::parse(source, &text);
    let body = super::super::ast::Program::root(&parsed.syntax)
        .body()
        .unwrap();
    let names: Vec<_> = body
        .bindings()
        .map(|binding| {
            let range = binding.name().unwrap().range();
            &text[range.start as usize..range.end as usize]
        })
        .collect();
    assert_eq!(names, ["before", "broken", "after"]);
    assert_eq!(parsed.diagnostics.len(), 1, "{:?}", parsed.diagnostics);
    let issue = &parsed.diagnostics[0];
    assert!(issue.message.contains(']'));
    let location = issue.labels[0].location;
    assert_eq!(location.start as usize, text.find(')').unwrap());
    assert_eq!(location.start, location.end);
}

#[test]
fn unclosed_multiline_string_does_not_invent_following_declarations() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/hir-lower/unclosed-string.telora");
    let text = std::fs::read_to_string(path).unwrap();
    let mut sources = crate::source::SourceDatabase::default();
    let source = sources.add("unclosed.telora", &text);
    let parsed = super::parse(source, &text);
    let body = super::super::ast::Program::root(&parsed.syntax)
        .body()
        .unwrap();
    assert_eq!(body.bindings().count(), 0);
    assert!(body.result().is_some());
    assert!(
        parsed
            .diagnostics
            .iter()
            .any(|issue| issue.message.contains("unclosed string"))
    );
    let content: Vec<_> =
        super::super::ast::SyntaxNode::new(&parsed.syntax, super::super::cst::NodeRef::ROOT)
            .descendants_and_self()
            .filter_map(|node| node.token())
            .filter(|token| token.kind() == super::Token::StringText)
            .map(|token| token.range())
            .collect();
    assert_eq!(content.len(), 1);
    assert_eq!(content[0].start, 1);
    assert_eq!(content[0].end as usize, text.len());
}

#[test]
fn long_generated_tokens_can_cancel_without_publishing_synthetic_eof() {
    for (prefix, suffix) in [("\"", "\""), ("#", "\n1"), ("", "")] {
        let text = format!("{prefix}{}{suffix}", "x".repeat(2 * 1024 * 1024));
        let document = crate::document::DocumentText::new(&text);
        let mut sources = crate::source::SourceDatabase::default();
        let source = sources.add("long-token.telora", &text);
        let mut calls = 0;
        // Allow source indexing to finish, then cancel during parser input.
        let stop_at = 5 + document.chunks().count().div_ceil(256);
        let parsed = super::super::parse_document_cancellable(source, &document, &mut || {
            calls += 1;
            // Deliberately transient: once observed, cancellation must latch.
            calls == stop_at
        });
        assert!(parsed.is_none(), "{prefix:?}");
        assert_eq!(calls, stop_at);
        let parsed = super::super::parse_document_cancellable(source, &document, &mut || false)
            .expect("normal parse after cancellation");
        assert!(parsed.diagnostics.is_empty(), "{prefix:?}");
    }
}

#[test]
fn cancellation_stops_structural_validation_between_bindings() {
    let text = "let a = 1; let b = 2;";
    let mut sources = crate::source::SourceDatabase::default();
    let source = sources.add("cancel-validation.telora", text);
    let parsed = super::parse(source, text);
    assert!(parsed.diagnostics.is_empty());
    let mut calls = 0;
    let result = super::super::ast::validate_cancellable(source, &parsed.syntax, &mut || {
        calls += 1;
        calls == 3
    });
    assert!(result.is_none());
    assert_eq!(calls, 3);
    assert!(super::super::ast::validate(source, &parsed.syntax).is_empty());
}

#[test]
fn cancelled_document_does_not_publish_partial_syntax() {
    let text = format!("r##\"{}\"##", "x".repeat(2 * 1024 * 1024));
    let document = crate::document::DocumentText::new(&text);
    let mut sources = crate::source::SourceDatabase::default();
    let source = sources.add("cancel.telora", &text);
    let mut calls = 0;
    let result = super::super::parse_document_cancellable(source, &document, &mut || {
        calls += 1;
        calls == 2
    });
    assert!(result.is_none());
    assert_eq!(calls, 2);
    let result = super::super::parse_document_cancellable(source, &document, &mut || false)
        .expect("fresh parse after cancellation");
    assert!(result.diagnostics.is_empty());
}

#[test]
fn cancellation_stops_token_extraction_and_projection() {
    let text = "1 + 2";
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_telora::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(text, None).unwrap();
    let mut diagnostics = vec![];
    let result = super::tokens::extract_cancellable(
        &tree,
        text.len(),
        |range| std::borrow::Cow::Borrowed(&text[range]),
        &mut diagnostics,
        &mut || true,
    );
    assert!(result.is_none());
    let (tokens, spans) = super::tokens::extract(
        &tree,
        text.len(),
        |range| std::borrow::Cow::Borrowed(&text[range]),
        &mut diagnostics,
    );
    let mut sources = crate::source::SourceDatabase::default();
    let source = sources.add("cancel-projection.telora", text);
    assert!(
        super::project_cancellable(
            source,
            text.len(),
            tree,
            tokens,
            spans,
            diagnostics,
            &mut || true,
        )
        .is_none()
    );
}

#[test]
fn cst_leaf_token_mapping_covers_standard_library() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("modules");
    let mut directories = vec![root];
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_telora::LANGUAGE.into())
        .unwrap();
    let mut unsupported = std::collections::BTreeSet::new();
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "telora") {
                continue;
            }
            let source = std::fs::read_to_string(path).unwrap();
            let tree = parser.parse(&source, None).unwrap();
            assert!(!tree.root_node().has_error());
            let mut pending = vec![tree.root_node()];
            while let Some(node) = pending.pop() {
                if node.child_count() == 0
                    && super::tokens::leaf(node.kind()).is_none()
                    && !matches!(
                        node.kind(),
                        "escape_sequence" | "raw_start" | "raw_text" | "raw_end"
                    )
                {
                    unsupported.insert(node.kind().to_owned());
                }
                let mut cursor = node.walk();
                pending.extend(node.children(&mut cursor));
            }
        }
    }
    assert!(unsupported.is_empty(), "{unsupported:?}");
}

#[test]
fn trivia_only_module_has_an_empty_body() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/hir-lower/trivia-only.telora"
    ))
    .unwrap();
    let mut sources = crate::source::SourceDatabase::default();
    let source = sources.add("trivia.telora", &text);
    let parsed = super::parse(source, &text);
    assert!(parsed.diagnostics.is_empty());
    let program = crate::syntax::telora::ast::Program::root(&parsed.syntax);
    let body = program.body().unwrap();
    assert_eq!(body.bindings().count(), 0);
    assert!(body.result().is_none());
}

#[test]
fn static_module_syntax_keeps_declarations_paths_and_data_imports_distinct() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/hir-lower/static-modules.telora"
    ))
    .unwrap();
    let mut sources = crate::source::SourceDatabase::default();
    let source = sources.add("static-modules.telora", &text);
    let parsed = super::parse(source, &text);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);

    let root = super::super::ast::SyntaxNode::new(&parsed.syntax, super::super::cst::NodeRef::ROOT);
    let mut counts = std::collections::BTreeMap::new();
    for node in root.descendants_and_self() {
        if let Some(rule) = node.rule() {
            *counts.entry(format!("{rule:?}")).or_insert(0usize) += 1;
        }
    }
    assert_eq!(counts.get("module_declaration"), Some(&1));
    assert_eq!(counts.get("use_binding"), Some(&3));
    assert_eq!(counts.get("use_selector"), Some(&1));
    assert_eq!(counts.get("data_binding"), Some(&2));
    assert_eq!(counts.get("data_import"), Some(&2));
    assert_eq!(counts.get("data_format"), Some(&2));
    assert_eq!(counts.get("static_path_expr"), Some(&1));
    assert_eq!(counts.get("static_path"), Some(&3));

    let body = super::super::ast::Program::root(&parsed.syntax)
        .body()
        .unwrap();
    let bindings: Vec<_> = body.bindings().collect();
    assert!(matches!(bindings[0], super::super::ast::Binding::Module(_)));
    assert!(matches!(bindings[1], super::super::ast::Binding::Use(_)));
    assert!(matches!(bindings[2], super::super::ast::Binding::Use(_)));
    assert!(matches!(bindings[4], super::super::ast::Binding::Data(_)));
    assert!(matches!(bindings[5], super::super::ast::Binding::Data(_)));

    let super::super::ast::Binding::Module(module) = bindings[0] else {
        unreachable!()
    };
    let name = module.name().unwrap().range();
    assert_eq!(&text[name.start as usize..name.end as usize], "query");

    let super::super::ast::Binding::Data(config) = bindings[4] else {
        unreachable!()
    };
    assert!(config.annotation().is_some());
    let import = config.import().unwrap();
    assert_eq!(import.format().unwrap().kind(), super::Token::Json);
    assert!(import.source().is_some());

    let super::super::ast::Binding::Data(defaults) = bindings[5] else {
        unreachable!()
    };
    assert!(defaults.annotation().is_none());
    assert_eq!(
        defaults.import().unwrap().format().unwrap().kind(),
        super::Token::Json
    );
}

#[test]
fn direct_use_alias_can_precede_another_binding_on_the_same_line() {
    let text = "use std::test as test; type Box(T) = struct {value: T};";
    let mut sources = crate::source::SourceDatabase::default();
    let source = sources.add("use-alias.telora", text);
    let parsed = super::parse(source, text);
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
}

#[test]
fn deep_parse_projection_and_drop_use_a_bounded_native_stack() {
    std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(|| {
            let depth = 20_000;
            for (open, close) in [("(", ")"), ("[", "]")] {
                let started = std::time::Instant::now();
                let text = format!("{}1{}", open.repeat(depth), close.repeat(depth));
                let mut sources = crate::source::SourceDatabase::default();
                let source = sources.add("deep.telora", &text);
                let parsed = super::parse(source, &text);
                assert!(parsed.diagnostics.is_empty());
                let count = crate::syntax::telora::ast::SyntaxNode::new(
                    &parsed.syntax,
                    super::super::cst::NodeRef::ROOT,
                )
                .descendants_and_self()
                .count();
                assert!(count > depth);
                drop(parsed);
                eprintln!("deep {open}{close}: {:?}", started.elapsed());
            }
            // A long operator chain has shallow delimiters but a deep CST.
            let text = format!("{}1", "1 + ".repeat(depth));
            let started = std::time::Instant::now();
            let mut sources = crate::source::SourceDatabase::default();
            let source = sources.add("operators.telora", &text);
            let parsed = super::parse(source, &text);
            assert!(parsed.diagnostics.is_empty());
            drop(parsed);
            eprintln!("operator chain: {:?}", started.elapsed());
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn rope_chunks_preserve_cst_spans_and_tokens() {
    let text = format!("r##\"{}\"##", "正文\r\n".repeat(4096));
    let document = crate::document::DocumentText::new(&text);
    assert!(document.chunks().count() > 1);
    let mut sources = crate::source::SourceDatabase::default();
    let source = sources.add("chunked.telora", &text);
    let contiguous = super::parse(source, &text);
    let chunked = super::parse_document(source, &document);
    assert!(contiguous.diagnostics.is_empty());
    assert!(chunked.diagnostics.is_empty());
    let fingerprint = |cst: &super::CstData| {
        crate::syntax::telora::ast::SyntaxNode::new(cst, super::super::cst::NodeRef::ROOT)
            .descendants_and_self()
            .map(|node| {
                (
                    node.rule(),
                    node.token().map(|token| token.kind()),
                    node.range(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        fingerprint(&contiguous.syntax),
        fingerprint(&chunked.syntax)
    );
}
