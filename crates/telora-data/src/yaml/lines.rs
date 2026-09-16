use super::lexer::LineToken;
pub(super) use super::lexer::{mapping, uncomment};
use alloc::vec::Vec;
use logos::Logos;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Line {
    pub start: usize,
    pub end: usize,
    pub indent: usize,
    pub tab_indent: bool,
    pub trivia: Option<bool>,
}

/// Chunk boundaries do not end lines; CRLF can straddle two chunks.
pub(super) fn index<'a>(chunks: impl Iterator<Item = &'a str>) -> Vec<Line> {
    let mut lines = Vec::new();
    let (mut start, mut at, mut indent) = (0, 0, 0);
    let (mut leading, mut cr) = (true, false);
    let mut tab_indent = false;
    for mut chunk in chunks {
        if chunk.is_empty() {
            continue;
        }
        if cr && chunk.starts_with('\n') {
            at += 1;
            start = at;
            chunk = &chunk[1..];
        }
        cr = false;
        let base = at;
        let mut lexer = LineToken::lexer(chunk);
        while let Some(token) = lexer.next() {
            let token = token.expect("complete line alphabet");
            let range = lexer.span();
            at = base + range.end;
            match token {
                LineToken::Eol => {
                    lines.push(Line {
                        start,
                        end: base + range.start,
                        indent,
                        tab_indent,
                        trivia: None,
                    });
                    start = at;
                    indent = 0;
                    leading = true;
                    tab_indent = false;
                    cr = lexer.slice() == "\r";
                }
                LineToken::Spaces if leading => {
                    indent += range.len();
                    cr = false;
                }
                _ => {
                    tab_indent |= leading && token == LineToken::Tabs;
                    leading = false;
                    cr = false;
                }
            }
        }
    }
    if start < at {
        lines.push(Line {
            start,
            end: at,
            indent,
            tab_indent,
            trivia: None,
        });
    }
    lines
}
