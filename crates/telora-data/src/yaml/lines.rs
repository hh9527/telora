use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Line {
    pub start: usize,
    pub end: usize,
    pub indent: usize,
    pub tab_indent: bool,
    pub trivia: Option<bool>,
}

/// Scan every input byte once. CRLF may straddle chunks; neither byte is part
/// of a line's content. UTF-8 bytes do not need decoding for these boundaries.
pub(super) fn index<'a>(chunks: impl Iterator<Item = &'a str>) -> Vec<Line> {
    let mut lines = Vec::new();
    let (mut start, mut at, mut indent) = (0, 0, 0);
    let (mut leading, mut cr) = (true, false);
    let mut tab_indent = false;
    for chunk in chunks {
        for byte in chunk.bytes() {
            if cr && byte == b'\n' {
                at += 1;
                start = at;
                cr = false;
                continue;
            }
            cr = byte == b'\r';
            if matches!(byte, b'\r' | b'\n') {
                lines.push(Line {
                    start,
                    end: at,
                    indent,
                    tab_indent,
                    trivia: None,
                });
                start = at + 1;
                indent = 0;
                leading = true;
                tab_indent = false;
            } else if leading && byte == b' ' {
                indent += 1;
            } else {
                tab_indent |= leading && byte == b'\t';
                leading = false;
            }
            at += 1;
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

/// Find structural colons/comments without treating escaped quotes or doubled
/// single quotes as the end of a quoted scalar.
pub(super) fn mapping(text: &str) -> Option<usize> {
    let (mut quote, mut escaped, mut depth) = (None, false, 0usize);
    let mut chars = text.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            if q == '"' && ch == '\\' {
                escaped = true;
            } else if ch == q {
                if q == '\'' && chars.peek().is_some_and(|(_, c)| *c == '\'') {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '[' | '{' => depth += 1,
            ']' | '}' => depth = depth.saturating_sub(1),
            '#' if at == 0 || text[..at].ends_with(char::is_whitespace) => return None,
            ':' if depth == 0
                && text[at + 1..]
                    .chars()
                    .next()
                    .is_none_or(char::is_whitespace) =>
            {
                return Some(at);
            }
            _ => {}
        }
    }
    None
}

pub(super) fn uncomment(text: &str) -> &str {
    let (mut quote, mut escaped) = (None, false);
    let mut chars = text.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(q) = quote {
            if q == '"' && ch == '\\' {
                escaped = true;
            } else if ch == q {
                if q == '\'' && chars.peek().is_some_and(|(_, c)| *c == '\'') {
                    chars.next();
                } else {
                    quote = None;
                }
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch == '#' && (at == 0 || text[..at].ends_with(char::is_whitespace)) {
            return &text[..at];
        }
    }
    text
}
