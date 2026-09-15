//! Borrowed source input and cooperative parser cancellation.
use crate::document::DocumentText;
use std::cell::{Cell, RefCell};
use tree_sitter::{ParseOptions, ParseState, Parser, Tree};

fn parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_telora::LANGUAGE.into())
        .expect("bundled grammar ABI");
    parser
}

pub(super) fn parse(text: &str) -> Tree {
    parser().parse(text, None).expect("uncancelled parse")
}

pub(super) fn parse_document(
    text: &DocumentText,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<Tree> {
    if cancelled() {
        return None;
    }
    let mut offset = 0;
    let mut chunks = Vec::new();
    for chunk in text.chunks() {
        if chunks.len() % 256 == 0 && cancelled() {
            return None;
        }
        chunks.push((offset, chunk));
        offset += chunk.len();
    }
    let mut parser = parser();
    let cancelled = RefCell::new(cancelled);
    let stopped = Cell::new(false);
    let check = || {
        if stopped.get() {
            return true;
        }
        let stop = (*cancelled.borrow_mut())();
        stopped.set(stop);
        stop
    };
    let mut progress = |_: &ParseState| check();
    let tree = parser.parse_with_options(
        &mut |offset, _| {
            // Long tokens span input reads between parser progress checks.
            // Synthetic EOF after cancellation must never publish a tree.
            if check() || offset >= text.byte_len() {
                return &[][..];
            }
            let index = chunks.partition_point(|(start, _)| *start <= offset) - 1;
            let (start, chunk) = chunks[index];
            let bytes = &chunk.as_bytes()[offset - start..];
            &bytes[..bytes.len().min(4096)]
        },
        None,
        Some(ParseOptions::new().progress_callback(&mut progress)),
    );
    if stopped.get() { None } else { tree }
}
