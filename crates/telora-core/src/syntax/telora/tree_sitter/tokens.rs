use super::Token;
use super::{Diagnostic, Span};
use std::borrow::Cow;

pub(super) fn extract<'a>(
    tree: &tree_sitter::Tree,
    byte_len: usize,
    text: impl Fn(Span) -> Cow<'a, str>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<Token>, Vec<Span>) {
    extract_cancellable(tree, byte_len, text, diagnostics, &mut || false)
        .expect("uncancelled token extraction")
}

pub(super) fn extract_cancellable<'a>(
    tree: &tree_sitter::Tree,
    byte_len: usize,
    text: impl Fn(Span) -> Cow<'a, str>,
    diagnostics: &mut Vec<Diagnostic>,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<(Vec<Token>, Vec<Span>)> {
    let mut tokens = vec![];
    let mut spans = vec![];
    let mut pending = vec![(tree.root_node(), false)];
    let mut children = vec![];
    let mut end = 0;
    let mut visited = 0usize;
    while let Some((node, in_concat)) = pending.pop() {
        if visited % 256 == 0 && cancelled() {
            return None;
        }
        visited += 1;
        if node.is_missing() {
            continue;
        }
        let raw = node.kind() == "raw_string";
        if node.child_count() != 0 && !raw {
            children.clear();
            let mut cursor = node.walk();
            children.extend(node.children(&mut cursor));
            let in_concat = node.kind() == "concat_string";
            pending.extend(children.iter().rev().map(|child| (*child, in_concat)));
            continue;
        }
        let span = node.byte_range();
        if span.is_empty() {
            continue;
        }
        if span.start > end {
            let gap = end..span.start;
            let whitespace = text(gap.clone()).chars().all(char::is_whitespace);
            if !whitespace && !tree.root_node().has_error() {
                diagnostics.push(super::issue(
                    gap.clone(),
                    "unmapped source range in Tree-sitter CST".into(),
                ));
            }
            tokens.push(if whitespace {
                Token::Whitespace
            } else {
                Token::Error
            });
            spans.push(gap);
        }
        let token = if raw {
            Token::RawString
        } else if node.kind() == "escape_sequence" {
            escape(&text(span.clone()), in_concat)
        } else {
            super::kinds::token(node).unwrap_or_else(|| {
                if !node.is_error() {
                    diagnostics.push(super::issue(
                        span.clone(),
                        format!("unsupported Tree-sitter token {}", node.kind()),
                    ));
                }
                Token::Error
            })
        };
        let message = match token {
            Token::UnknownEscapeSequence => Some("unsupported string escape"),
            Token::UnterminatedEscapeSequence => Some("unterminated string escape"),
            _ => None,
        };
        if let Some(message) = message {
            diagnostics.push(super::issue(span.clone(), message.into()));
        }
        end = span.end;
        tokens.push(token);
        spans.push(span);
    }
    if end < byte_len {
        let span = end..byte_len;
        let whitespace = text(span.clone()).chars().all(char::is_whitespace);
        if !whitespace && !tree.root_node().has_error() {
            diagnostics.push(super::issue(
                span.clone(),
                "unmapped source range in Tree-sitter CST".into(),
            ));
        }
        tokens.push(if whitespace {
            Token::Whitespace
        } else {
            Token::Error
        });
        spans.push(span);
    }
    Some((tokens, spans))
}

fn escape(text: &str, concat: bool) -> Token {
    let value = text.strip_prefix('\\').unwrap_or(text);
    if value.is_empty() {
        return Token::UnterminatedEscapeSequence;
    }
    let valid = matches!(value, "0" | "n" | "r" | "t" | "\\")
        || value == if concat { "`" } else { "\"" }
        || value.chars().all(char::is_whitespace)
        || value.strip_prefix('x').is_some_and(|digits| {
            digits.len() == 2 && digits.bytes().all(|b| b.is_ascii_hexdigit())
        })
        || value
            .strip_prefix("u{")
            .and_then(|value| value.strip_suffix('}'))
            .is_some_and(|digits| {
                (1..=6).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_hexdigit())
            });
    if valid {
        Token::EscapeSequence
    } else {
        Token::UnknownEscapeSequence
    }
}

/// Classify an already parsed leaf. No source text is scanned here; escape
/// validation and raw-string aggregation are handled separately.
pub(super) fn leaf(kind: &str) -> Option<Token> {
    Some(match kind {
        "let" => Token::Let,
        "decl" => Token::Decl,
        "def" => Token::Def,
        "do" => Token::Do,
        "native" => Token::Native,
        "for" => Token::For,
        "type" => Token::Type,
        "trait" => Token::Trait,
        "impl" => Token::Impl,
        "fn" => Token::Fn,
        "Fn" => Token::FunctionType,
        "if" => Token::If,
        "else" => Token::Else,
        "match" => Token::Match,
        "return" => Token::Return,
        "mod" => Token::Mod,
        "use" => Token::Use,
        "data" => Token::Data,
        "crate" => Token::Crate,
        "self" => Token::SelfModule,
        "super" => Token::Super,
        "import" => Token::Import,
        "export" => Token::Export,
        "as" => Token::As,
        "section_lparen" => Token::SectionLParen,
        "(" => Token::LParen,
        ")" => Token::RParen,
        "{" => Token::LBrace,
        "}" => Token::RBrace,
        "[" => Token::LBracket,
        "]" => Token::RBracket,
        "," => Token::Comma,
        ":" => Token::Colon,
        ";" => Token::Semicolon,
        "..." => Token::Ellipsis,
        "." => Token::Dot,
        "::" => Token::DoubleColon,
        "@" => Token::At,
        "!" => Token::Bang,
        "?" => Token::Question,
        "+" => Token::Plus,
        "-" => Token::Minus,
        "*" => Token::Star,
        "/" => Token::Slash,
        "%" => Token::Percent,
        "<" => Token::Less,
        "<=" => Token::LessEqual,
        ">" => Token::Greater,
        ">=" => Token::GreaterEqual,
        "==" => Token::EqualEqual,
        "!=" => Token::BangEqual,
        "=" => Token::Equal,
        "&" => Token::BitAnd,
        "<~" => Token::StructUpdate,
        "|" => Token::BitOr,
        "^" => Token::BitXor,
        "&&" => Token::AndAnd,
        "||" => Token::OrOr,
        "->" => Token::Arrow,
        "=>" => Token::FatArrow,
        "|>" => Token::Pipe,
        "int_expr" | "int_pattern" | "native_type_binding_token1" => Token::Int,
        "float_expr" | "float_pattern" | "float_expr_token1" => Token::Float,
        "interpreter" => Token::Interpreter,
        "quote_start" | "quote_end" => Token::DoubleQuote,
        "`" => Token::Backtick,
        "string_text" | "concat_fragment" => Token::StringText,
        "\\{" => Token::InterpolationStart,
        "bytes_expr" => Token::Bytes,
        "placeholder" => Token::Placeholder,
        "indexed_placeholder" => Token::IndexedPlaceholder,
        "identifier" => Token::Identifier,
        "struct" => Token::StructInitializer,
        "enum" => Token::EnumInitializer,
        "json" => Token::Json,
        "yaml" => Token::Yaml,
        "toml" => Token::Toml,
        "spaces" | "tabs" | "newline" => Token::Whitespace,
        "comment" => Token::Comment,
        _ => return None,
    })
}
