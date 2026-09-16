//! Logos recognizes local runs. The caller owns chunk, quoting and parser state.
use logos::Logos;

#[derive(Logos, Clone, Copy, Debug, PartialEq)]
pub(super) enum Normal {
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("=")]
    Equals,
    #[token(",")]
    Comma,
    #[token("\"")]
    BasicQuote,
    #[token("'")]
    LiteralQuote,
    #[regex(r"[A-Za-z0-9_+:.-]+")]
    Atom,
    #[regex(r"[ \t]+")]
    Space,
    #[regex(r"\r\n|\r|\n")]
    Newline,
    #[regex(r"#[^\r\n]*", allow_greedy = true)]
    Comment,
}

#[derive(Logos)]
enum Key {
    #[regex(r"[A-Za-z0-9_-]+")]
    Text,
}

#[derive(Logos)]
enum Basic {
    #[token("\"")]
    Quote,
    #[token("\\")]
    Escape,
    #[regex(r"\r\n|\r|\n")]
    Newline,
    #[regex(r#"[^"\\\x00-\x08\x0a-\x1f\x7f]+"#)]
    Text,
}

#[derive(Logos)]
enum Literal {
    #[token("'")]
    Quote,
    #[regex(r"\r\n|\r|\n")]
    Newline,
    #[regex(r"[^'\x00-\x08\x0a-\x1f\x7f]+")]
    Text,
}

pub(super) fn normal(text: &str) -> Option<(Normal, usize)> {
    let mut lexer = Normal::lexer(text);
    let kind = lexer.next()?.ok()?;
    Some((kind, lexer.span().end))
}
pub(super) fn key(text: &str) -> usize {
    let mut lexer = Key::lexer(text);
    if matches!(lexer.next(), Some(Ok(Key::Text))) {
        lexer.span().end
    } else {
        0
    }
}
pub(super) fn text(text: &str, basic: bool) -> usize {
    if basic {
        let mut lexer = Basic::lexer(text);
        if matches!(lexer.next(), Some(Ok(Basic::Text))) {
            lexer.span().end
        } else {
            0
        }
    } else {
        let mut lexer = Literal::lexer(text);
        if matches!(lexer.next(), Some(Ok(Literal::Text))) {
            lexer.span().end
        } else {
            0
        }
    }
}
