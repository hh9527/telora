use super::parser::{Diagnostic, Span};
use codespan_reporting::diagnostic::Label;
use logos::{Lexer, Logos};

#[derive(Debug, Clone, PartialEq, Default)]
pub enum LexerError {
    #[default]
    Invalid,
}

impl LexerError {
    pub fn into_diagnostic(self, span: Span) -> Diagnostic {
        match self {
            Self::Invalid => Diagnostic::error()
                .with_message("invalid token")
                .with_label(Label::primary((), span)),
        }
    }
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Token {
    EOF,
    Let,
    Decl,
    Def,
    Native,
    Option,
    For,
    Type,
    Fn,
    FunctionType,
    Interpreter,
    If,
    Else,
    Match,
    Return,
    Import,
    Export,
    As,
    SectionLParen,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Semicolon,
    Ellipsis,
    Dot,
    At,
    Bang,
    Question,
    Plus,
    Minus,
    Star,
    Slash,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    EqualEqual,
    BangEqual,
    Equal,
    BitAnd,
    BitOr,
    BitXor,
    AndAnd,
    OrOr,
    Arrow,
    FatArrow,
    Pipe,
    Int,
    Float,
    DoubleQuote,
    Backtick,
    RawString,
    StringText,
    EscapeSequence,
    UnknownEscapeSequence,
    UnterminatedEscapeSequence,
    InterpolationStart,
    Bytes,
    Atom,
    Placeholder,
    IndexedPlaceholder,
    Identifier,
    Whitespace,
    Comment,
    Error,
}

#[derive(Logos, Debug, PartialEq, Copy, Clone)]
#[logos(error = LexerError, extras = LexerState)]
enum NormalToken {
    #[token("let")]
    Let,
    #[token("decl")]
    Decl,
    #[token("def")]
    Def,
    #[token("native")]
    Native,
    #[token("option")]
    Option,
    #[token("for")]
    For,
    #[token("type")]
    Type,
    #[token("fn")]
    Fn,
    #[token("Fn")]
    FunctionType,
    #[token("interpreter")]
    Interpreter,
    #[token("if")]
    If,
    #[token("else")]
    Else,
    #[token("match")]
    Match,
    #[token("return")]
    Return,
    #[token("import")]
    Import,
    #[token("export")]
    Export,
    #[token("as")]
    As,
    #[token("\\(")]
    SectionLParen,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("{")]
    LBrace,
    #[token("}")]
    RBrace,
    #[token("[")]
    LBracket,
    #[token("]")]
    RBracket,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token(";")]
    Semicolon,
    #[token("...")]
    Ellipsis,
    #[token(".")]
    Dot,
    #[token("@")]
    At,
    #[token("!")]
    Bang,
    #[token("?")]
    Question,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("<")]
    Less,
    #[token("<=")]
    LessEqual,
    #[token(">")]
    Greater,
    #[token(">=")]
    GreaterEqual,
    #[token("==")]
    EqualEqual,
    #[token("!=")]
    BangEqual,
    #[token("->")]
    Arrow,
    #[token("=")]
    Equal,
    #[token("&")]
    BitAnd,
    #[token("|")]
    BitOr,
    #[token("^")]
    BitXor,
    #[token("&&")]
    AndAnd,
    #[token("||")]
    OrOr,
    #[token("=>")]
    FatArrow,
    #[token("|>")]
    Pipe,
    #[regex(r"[0-9]+")]
    Int,
    #[regex(r"[0-9]+(\.[0-9]+([eE][+-]?[0-9]+)?|[eE][+-]?[0-9]+)")]
    Float,
    #[token("\"")]
    DoubleQuote,
    #[token("`")]
    Backtick,
    #[regex(r##"r#*\""##, scan_raw_string, priority = 5)]
    RawString,
    #[regex(r#"b\"([^\"\\]|\\.)*\""#)]
    Bytes,
    #[regex(r"'[A-Za-z_][A-Za-z0-9_]*")]
    Atom,
    #[token("_", priority = 4)]
    Placeholder,
    #[regex(r"_[0-9]+", priority = 4)]
    IndexedPlaceholder,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Identifier,
    #[regex(r"[ \t\r\n]+")]
    Whitespace,
    #[regex(r"#[^\r\n]*", allow_greedy = true)]
    Comment,
}

#[derive(Logos, Debug, PartialEq, Copy, Clone)]
#[logos(error = LexerError, extras = LexerState)]
enum StringToken {
    #[token("\"")]
    DoubleQuote,
    #[regex(
        r#"\\(0|[nrt"\\]|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]{1,6}\}|\r?\n[ \t\r\n]*)"#,
        priority = 4
    )]
    EscapeSequence,
    #[regex(r#"\\[^\r\n]"#, priority = 3)]
    UnknownEscapeSequence,
    #[token("\\", priority = 1)]
    UnterminatedEscapeSequence,
    #[regex(r#"[^\"\\]+"#)]
    StringText,
}

#[derive(Logos, Debug, PartialEq, Copy, Clone)]
#[logos(error = LexerError, extras = LexerState)]
enum ConcatToken {
    #[token("`")]
    Backtick,
    #[token("\\{", priority = 5)]
    InterpolationStart,
    #[regex(
        r#"\\(0|[nrt`\\]|x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f]{1,6}\}|\r?\n[ \t\r\n]*)"#,
        priority = 4
    )]
    EscapeSequence,
    #[regex(r#"\\[^\r\n]"#, priority = 3)]
    UnknownEscapeSequence,
    #[token("\\", priority = 1)]
    UnterminatedEscapeSequence,
    #[regex(r#"[^`\\]+"#)]
    StringText,
}

#[derive(Clone, Copy, Debug)]
enum Context {
    Root,
    Interpolation { brace_depth: usize },
    String,
    Concat,
}

#[derive(Debug)]
struct LexerState {
    contexts: Vec<Context>,
}

impl Default for LexerState {
    fn default() -> Self {
        Self {
            contexts: vec![Context::Root],
        }
    }
}

enum ActiveLexer<'source> {
    Normal(Lexer<'source, NormalToken>),
    String(Lexer<'source, StringToken>),
    Concat(Lexer<'source, ConcatToken>),
}

fn scan_raw_string(lexer: &mut Lexer<'_, NormalToken>) -> bool {
    let hashes = lexer.slice().len().saturating_sub(2);
    if hashes > 255 {
        return true;
    }
    let terminator = format!("\"{}", "#".repeat(hashes));
    if let Some(index) = lexer.remainder().find(&terminator) {
        lexer.bump(index + terminator.len());
    } else {
        lexer.bump(lexer.remainder().len());
    }
    true
}

pub fn tokenize(source: &str, diags: &mut Vec<Diagnostic>) -> (Vec<Token>, Vec<Span>) {
    tokenize_internal(source, diags, true)
}

fn tokenize_internal(
    source: &str,
    diags: &mut Vec<Diagnostic>,
    contextualize: bool,
) -> (Vec<Token>, Vec<Span>) {
    let mut tokens = Vec::new();
    let mut spans = Vec::new();
    let mut active = ActiveLexer::Normal(NormalToken::lexer(source));

    loop {
        let (token, span, error, next) = match active {
            ActiveLexer::Normal(mut lexer) => {
                let Some(result) = lexer.next() else {
                    break;
                };
                let span = lexer.span();
                let (token, mut error) = match result {
                    Ok(token) => (token.into(), false),
                    Err(_) => (Token::Error, true),
                };
                if token == Token::RawString && raw_string_error(lexer.slice()).is_some() {
                    error = true;
                }
                let context = *lexer
                    .extras
                    .contexts
                    .last()
                    .expect("lexer has a root context");
                let next = match (context, token) {
                    (Context::Root | Context::Interpolation { .. }, Token::DoubleQuote) => {
                        lexer.extras.contexts.push(Context::String);
                        ActiveLexer::String(lexer.morph())
                    }
                    (Context::Root | Context::Interpolation { .. }, Token::Backtick) => {
                        lexer.extras.contexts.push(Context::Concat);
                        ActiveLexer::Concat(lexer.morph())
                    }
                    (Context::Interpolation { brace_depth }, Token::LBrace) => {
                        *lexer
                            .extras
                            .contexts
                            .last_mut()
                            .expect("interpolation context") = Context::Interpolation {
                            brace_depth: brace_depth + 1,
                        };
                        ActiveLexer::Normal(lexer)
                    }
                    (Context::Interpolation { brace_depth: 0 }, Token::RBrace) => {
                        lexer.extras.contexts.pop();
                        match lexer.extras.contexts.last() {
                            Some(Context::String) => ActiveLexer::String(lexer.morph()),
                            Some(Context::Concat) => ActiveLexer::Concat(lexer.morph()),
                            _ => unreachable!("interpolation belongs to a text context"),
                        }
                    }
                    (Context::Interpolation { brace_depth }, Token::RBrace) => {
                        *lexer
                            .extras
                            .contexts
                            .last_mut()
                            .expect("interpolation context") = Context::Interpolation {
                            brace_depth: brace_depth - 1,
                        };
                        ActiveLexer::Normal(lexer)
                    }
                    _ => ActiveLexer::Normal(lexer),
                };
                (token, span, error, next)
            }
            ActiveLexer::String(mut lexer) => {
                let Some(result) = lexer.next() else {
                    break;
                };
                let span = lexer.span();
                let (token, error) = match result {
                    Ok(token) => (token.into(), false),
                    Err(_) => (Token::Error, true),
                };
                let next = match token {
                    Token::DoubleQuote => {
                        lexer.extras.contexts.pop();
                        ActiveLexer::Normal(lexer.morph())
                    }
                    _ => ActiveLexer::String(lexer),
                };
                (token, span, error, next)
            }
            ActiveLexer::Concat(mut lexer) => {
                let Some(result) = lexer.next() else {
                    break;
                };
                let span = lexer.span();
                let (token, error) = match result {
                    Ok(token) => (token.into(), false),
                    Err(_) => (Token::Error, true),
                };
                let next = match token {
                    Token::Backtick => {
                        lexer.extras.contexts.pop();
                        ActiveLexer::Normal(lexer.morph())
                    }
                    Token::InterpolationStart => {
                        lexer
                            .extras
                            .contexts
                            .push(Context::Interpolation { brace_depth: 0 });
                        ActiveLexer::Normal(lexer.morph())
                    }
                    _ => ActiveLexer::Concat(lexer),
                };
                (token, span, error, next)
            }
        };
        let error = error || token_is_invalid_escape(token);
        if error {
            let message = match token {
                Token::UnknownEscapeSequence => "unsupported string escape",
                Token::UnterminatedEscapeSequence => "unterminated string escape",
                Token::RawString => {
                    raw_string_error(&source[span.clone()]).unwrap_or("invalid raw String")
                }
                _ => "invalid token",
            };
            diags.push(
                Diagnostic::error()
                    .with_message(message)
                    .with_label(Label::primary((), span.clone())),
            );
        }
        tokens.push(token);
        spans.push(span);
        active = next;
    }
    if contextualize {
        contextualize_projection_tokens(source, &mut tokens, &mut spans, None);
        contextualize_option_tokens(&mut tokens);
    }
    (tokens, spans)
}

fn contextualize_projection_tokens(
    source: &str,
    tokens: &mut Vec<Token>,
    spans: &mut Vec<Span>,
    mut previous_significant: Option<Token>,
) -> Option<Token> {
    let mut index = 0;
    while index < tokens.len() {
        let token = tokens[index];
        let span = spans[index].clone();
        if token == Token::Float
            && previous_significant == Some(Token::Dot)
            && source[span.clone()].contains('.')
        {
            let decimal = source[span.clone()]
                .find('.')
                .expect("Float token contains a decimal point");
            let dot = span.start + decimal;
            tokens.splice(index..=index, [Token::Int, Token::Dot, Token::Int]);
            spans.splice(
                index..=index,
                [span.start..dot, dot..dot + 1, dot + 1..span.end],
            );
            previous_significant = Some(Token::Int);
            index += 3;
            continue;
        }
        if !matches!(token, Token::Whitespace | Token::Comment) {
            previous_significant = Some(token);
        }
        index += 1;
    }
    previous_significant
}

fn contextualize_option_tokens(tokens: &mut [Token]) {
    for index in 0..tokens.len() {
        if tokens[index] != Token::Option {
            continue;
        }
        let next = tokens[index + 1..]
            .iter()
            .copied()
            .find(|token| !matches!(token, Token::Whitespace | Token::Comment));
        if !matches!(next, Some(Token::DoubleQuote | Token::RawString)) {
            tokens[index] = Token::Identifier;
        }
    }
}

pub fn tokenize_document(
    source: &crate::document::DocumentText,
    diags: &mut Vec<Diagnostic>,
) -> (Vec<Token>, Vec<Span>) {
    tokenize_fragments(source.chunks(), diags)
}

fn tokenize_fragments<'a>(
    fragments: impl IntoIterator<Item = &'a str>,
    diags: &mut Vec<Diagnostic>,
) -> (Vec<Token>, Vec<Span>) {
    let mut tokens = Vec::new();
    let mut spans = Vec::new();
    let mut pending = String::new();
    let mut pending_start = 0;
    let mut previous_significant = None;

    for fragment in fragments {
        pending.push_str(fragment);
        let mut local_diags = Vec::new();
        let (mut local_tokens, mut local_spans) =
            tokenize_internal(&pending, &mut local_diags, false);
        let commit = stable_root_prefix(&local_tokens, &local_spans, &pending);
        if commit == 0 {
            continue;
        }
        let committed_end = local_spans[commit - 1].end;
        local_tokens.truncate(commit);
        local_spans.truncate(commit);
        previous_significant = contextualize_projection_tokens(
            &pending,
            &mut local_tokens,
            &mut local_spans,
            previous_significant,
        );
        tokens.extend(local_tokens);
        spans.extend(
            local_spans
                .into_iter()
                .map(|span| pending_start + span.start..pending_start + span.end),
        );
        for mut diagnostic in local_diags {
            if diagnostic
                .labels
                .iter()
                .all(|label| label.range.end <= committed_end)
            {
                for label in &mut diagnostic.labels {
                    label.range =
                        pending_start + label.range.start..pending_start + label.range.end;
                }
                diags.push(diagnostic);
            }
        }
        pending.drain(..committed_end);
        pending_start += committed_end;
    }

    if !pending.is_empty() {
        let mut local_diags = Vec::new();
        let (mut local_tokens, mut local_spans) =
            tokenize_internal(&pending, &mut local_diags, false);
        contextualize_projection_tokens(
            &pending,
            &mut local_tokens,
            &mut local_spans,
            previous_significant,
        );
        tokens.extend(local_tokens);
        spans.extend(
            local_spans
                .into_iter()
                .map(|span| pending_start + span.start..pending_start + span.end),
        );
        for mut diagnostic in local_diags {
            for label in &mut diagnostic.labels {
                label.range = pending_start + label.range.start..pending_start + label.range.end;
            }
            diags.push(diagnostic);
        }
    }

    contextualize_option_tokens(&mut tokens);
    (tokens, spans)
}

fn stable_root_prefix(tokens: &[Token], spans: &[Span], source: &str) -> usize {
    let mut contexts = vec![Context::Root];
    let mut root_boundaries = Vec::new();
    for (index, token) in tokens.iter().copied().enumerate() {
        if token == Token::RawString
            && raw_string_error(&source[spans[index].clone()]) == Some("unterminated raw String")
        {
            break;
        }
        let context = *contexts.last().expect("lexer has a root context");
        match (context, token) {
            (Context::Root | Context::Interpolation { .. }, Token::DoubleQuote) => {
                contexts.push(Context::String);
            }
            (Context::String, Token::DoubleQuote) => {
                contexts.pop();
            }
            (Context::Root | Context::Interpolation { .. }, Token::Backtick) => {
                contexts.push(Context::Concat);
            }
            (Context::Concat, Token::Backtick) => {
                contexts.pop();
            }
            (Context::Concat, Token::InterpolationStart) => {
                contexts.push(Context::Interpolation { brace_depth: 0 });
            }
            (Context::Interpolation { brace_depth }, Token::LBrace) => {
                *contexts.last_mut().expect("interpolation context") = Context::Interpolation {
                    brace_depth: brace_depth + 1,
                };
            }
            (Context::Interpolation { brace_depth: 0 }, Token::RBrace) => {
                contexts.pop();
            }
            (Context::Interpolation { brace_depth }, Token::RBrace) => {
                *contexts.last_mut().expect("interpolation context") = Context::Interpolation {
                    brace_depth: brace_depth - 1,
                };
            }
            _ => {}
        }
        if contexts.len() == 1 && index + 9 <= tokens.len() {
            root_boundaries.push(index + 1);
        }
    }
    root_boundaries.last().copied().unwrap_or(0)
}

fn raw_string_error(text: &str) -> Option<&'static str> {
    let hashes = text[1..].bytes().take_while(|byte| *byte == b'#').count();
    if hashes > 255 {
        return Some("raw String delimiter exceeds 255 # characters");
    }
    let terminator = format!("\"{}", "#".repeat(hashes));
    (!text.ends_with(&terminator)).then_some("unterminated raw String")
}

impl From<NormalToken> for Token {
    fn from(token: NormalToken) -> Self {
        match token {
            NormalToken::Let => Self::Let,
            NormalToken::Decl => Self::Decl,
            NormalToken::Def => Self::Def,
            NormalToken::Native => Self::Native,
            NormalToken::Option => Self::Option,
            NormalToken::For => Self::For,
            NormalToken::Type => Self::Type,
            NormalToken::Fn => Self::Fn,
            NormalToken::FunctionType => Self::FunctionType,
            NormalToken::Interpreter => Self::Interpreter,
            NormalToken::If => Self::If,
            NormalToken::Else => Self::Else,
            NormalToken::Match => Self::Match,
            NormalToken::Return => Self::Return,
            NormalToken::Import => Self::Import,
            NormalToken::Export => Self::Export,
            NormalToken::As => Self::As,
            NormalToken::SectionLParen => Self::SectionLParen,
            NormalToken::LParen => Self::LParen,
            NormalToken::RParen => Self::RParen,
            NormalToken::LBrace => Self::LBrace,
            NormalToken::RBrace => Self::RBrace,
            NormalToken::LBracket => Self::LBracket,
            NormalToken::RBracket => Self::RBracket,
            NormalToken::Comma => Self::Comma,
            NormalToken::Colon => Self::Colon,
            NormalToken::Semicolon => Self::Semicolon,
            NormalToken::Ellipsis => Self::Ellipsis,
            NormalToken::Dot => Self::Dot,
            NormalToken::At => Self::At,
            NormalToken::Bang => Self::Bang,
            NormalToken::Question => Self::Question,
            NormalToken::Plus => Self::Plus,
            NormalToken::Minus => Self::Minus,
            NormalToken::Star => Self::Star,
            NormalToken::Slash => Self::Slash,
            NormalToken::Less => Self::Less,
            NormalToken::LessEqual => Self::LessEqual,
            NormalToken::Greater => Self::Greater,
            NormalToken::GreaterEqual => Self::GreaterEqual,
            NormalToken::EqualEqual => Self::EqualEqual,
            NormalToken::BangEqual => Self::BangEqual,
            NormalToken::Arrow => Self::Arrow,
            NormalToken::Equal => Self::Equal,
            NormalToken::BitAnd => Self::BitAnd,
            NormalToken::BitOr => Self::BitOr,
            NormalToken::BitXor => Self::BitXor,
            NormalToken::AndAnd => Self::AndAnd,
            NormalToken::OrOr => Self::OrOr,
            NormalToken::FatArrow => Self::FatArrow,
            NormalToken::Pipe => Self::Pipe,
            NormalToken::Int => Self::Int,
            NormalToken::Float => Self::Float,
            NormalToken::DoubleQuote => Self::DoubleQuote,
            NormalToken::Backtick => Self::Backtick,
            NormalToken::RawString => Self::RawString,
            NormalToken::Bytes => Self::Bytes,
            NormalToken::Atom => Self::Atom,
            NormalToken::Placeholder => Self::Placeholder,
            NormalToken::IndexedPlaceholder => Self::IndexedPlaceholder,
            NormalToken::Identifier => Self::Identifier,
            NormalToken::Whitespace => Self::Whitespace,
            NormalToken::Comment => Self::Comment,
        }
    }
}

impl From<StringToken> for Token {
    fn from(token: StringToken) -> Self {
        match token {
            StringToken::DoubleQuote => Self::DoubleQuote,
            StringToken::EscapeSequence => Self::EscapeSequence,
            StringToken::UnknownEscapeSequence => Self::UnknownEscapeSequence,
            StringToken::UnterminatedEscapeSequence => Self::UnterminatedEscapeSequence,
            StringToken::StringText => Self::StringText,
        }
    }
}

impl From<ConcatToken> for Token {
    fn from(token: ConcatToken) -> Self {
        match token {
            ConcatToken::Backtick => Self::Backtick,
            ConcatToken::InterpolationStart => Self::InterpolationStart,
            ConcatToken::EscapeSequence => Self::EscapeSequence,
            ConcatToken::UnknownEscapeSequence => Self::UnknownEscapeSequence,
            ConcatToken::UnterminatedEscapeSequence => Self::UnterminatedEscapeSequence,
            ConcatToken::StringText => Self::StringText,
        }
    }
}

fn token_is_invalid_escape(token: Token) -> bool {
    matches!(
        token,
        Token::UnknownEscapeSequence | Token::UnterminatedEscapeSequence
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_the_complete_comparison_operator_family() {
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize("< <= > >= == !=", &mut diagnostics);
        assert_eq!(
            tokens
                .into_iter()
                .filter(|token| *token != Token::Whitespace)
                .collect::<Vec<_>>(),
            vec![
                Token::Less,
                Token::LessEqual,
                Token::Greater,
                Token::GreaterEqual,
                Token::EqualEqual,
                Token::BangEqual,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn distinguishes_bitwise_boolean_and_pipeline_operators() {
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize("! != & && | || |> ^", &mut diagnostics);
        assert_eq!(
            tokens
                .into_iter()
                .filter(|token| *token != Token::Whitespace)
                .collect::<Vec<_>>(),
            vec![
                Token::Bang,
                Token::BangEqual,
                Token::BitAnd,
                Token::AndAnd,
                Token::BitOr,
                Token::OrOr,
                Token::Pipe,
                Token::BitXor,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn hash_is_the_only_line_comment_marker() {
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize("# comment\n//", &mut diagnostics);
        assert_eq!(
            tokens,
            vec![
                Token::Comment,
                Token::Whitespace,
                Token::Slash,
                Token::Slash,
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn option_is_contextual_before_a_string_key() {
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize(
            "let option = 1; option \"module.test\" {}; option",
            &mut diagnostics,
        );
        assert_eq!(
            tokens
                .iter()
                .copied()
                .filter(|token| matches!(token, Token::Option | Token::Identifier))
                .collect::<Vec<_>>(),
            vec![Token::Identifier, Token::Option, Token::Identifier]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn distinguishes_chained_projections_from_float_literals() {
        let mut diagnostics = Vec::new();
        let source = "pair.1.0 1.0 1.0.1 pair. 12.34";
        let (tokens, spans) = tokenize(source, &mut diagnostics);
        let significant = tokens
            .iter()
            .copied()
            .zip(spans.iter())
            .filter(|(token, _)| !matches!(token, Token::Whitespace | Token::Comment))
            .collect::<Vec<_>>();
        assert_eq!(
            significant
                .iter()
                .map(|(token, _)| *token)
                .collect::<Vec<_>>(),
            vec![
                Token::Identifier,
                Token::Dot,
                Token::Int,
                Token::Dot,
                Token::Int,
                Token::Float,
                Token::Float,
                Token::Dot,
                Token::Int,
                Token::Identifier,
                Token::Dot,
                Token::Int,
                Token::Dot,
                Token::Int,
            ]
        );
        assert_eq!(significant[2].1, &(5..6));
        assert_eq!(significant[3].1, &(6..7));
        assert_eq!(significant[4].1, &(7..8));
        assert_eq!(significant[11].1, &(25..27));
        assert_eq!(significant[12].1, &(27..28));
        assert_eq!(significant[13].1, &(28..30));
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn recognizes_float_exponent_notation_without_affecting_projections() {
        let mut diagnostics = Vec::new();
        let source = "1e3 1.25e-3 1.0E+8 pair.1.0 pair.1e2";
        let (tokens, spans) = tokenize(source, &mut diagnostics);
        let significant = tokens
            .iter()
            .copied()
            .zip(spans.iter())
            .filter(|(token, _)| !matches!(token, Token::Whitespace | Token::Comment))
            .map(|(token, span)| (token, &source[span.clone()]))
            .collect::<Vec<_>>();
        assert_eq!(
            significant,
            vec![
                (Token::Float, "1e3"),
                (Token::Float, "1.25e-3"),
                (Token::Float, "1.0E+8"),
                (Token::Identifier, "pair"),
                (Token::Dot, "."),
                (Token::Int, "1"),
                (Token::Dot, "."),
                (Token::Int, "0"),
                (Token::Identifier, "pair"),
                (Token::Dot, "."),
                (Token::Float, "1e2"),
            ]
        );
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn chunk_bridge_matches_contiguous_lexing() {
        let samples = [
            "#!/usr/bin/env -S telora run\nlet identifier = 123.456 # comment\nidentifier",
            r#"b\"bytes\" `text \{name} tail`"#,
            r####"r##"raw "quotes", \slashes and `ticks`"##"####,
            r#"`first \
                second \{name}`"#,
            "_12 |> transform\\(_1, 2)",
            "let 中 = \"emoji 😀 and escape \\n\"; 中",
            "let option = 1; option \"module.test\" {}; option",
            "let pair = (0, (1, 2)); pair.1.0; (pair. 1.0, 1.0.1)",
        ];
        for sample in samples {
            let mut expected_diags = Vec::new();
            let expected = tokenize(sample, &mut expected_diags);
            for split in sample
                .char_indices()
                .map(|(index, _)| index)
                .chain(std::iter::once(sample.len()))
            {
                let mut actual_diags = Vec::new();
                let actual = tokenize_fragments(
                    [sample.get(..split).unwrap(), sample.get(split..).unwrap()],
                    &mut actual_diags,
                );
                assert_eq!(actual, expected, "split at {split} in {sample:?}");
                assert_eq!(
                    actual_diags, expected_diags,
                    "split at {split} in {sample:?}"
                );
            }
        }

        let source = format!(
            "let value = \"{}\"; value",
            "long text with an escape \\n and interpolation-like text ".repeat(100)
        );
        let document = crate::document::DocumentText::new(&source);
        assert!(document.chunks().count() > 1);
        let mut expected_diags = Vec::new();
        let expected = tokenize(&source, &mut expected_diags);
        let mut actual_diags = Vec::new();
        let actual = tokenize_document(&document, &mut actual_diags);
        assert_eq!(actual, expected);
        assert_eq!(actual_diags, expected_diags);
    }

    #[test]
    fn recognizes_structured_string_slices_without_payloads() {
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(r#"`hi, \{name}\n`"#, &mut diagnostics);
        assert!(diagnostics.is_empty());
        assert_eq!(
            tokens,
            vec![
                Token::Backtick,
                Token::StringText,
                Token::InterpolationStart,
                Token::Identifier,
                Token::RBrace,
                Token::EscapeSequence,
                Token::Backtick,
            ]
        );
        assert_eq!(spans[1], 1..5);
        assert_eq!(spans[2], 5..7);
        assert_eq!(spans[5], 12..14);

        let (tokens, spans) = tokenize(r#"let x = "text""#, &mut diagnostics);
        let quote = tokens
            .iter()
            .position(|token| *token == Token::DoubleQuote)
            .unwrap();
        assert_eq!(spans[quote], 8..9);
        assert_eq!(spans[quote + 1], 9..13);
        assert_eq!(spans[quote + 2], 13..14);
    }

    #[test]
    fn recognizes_bare_and_indexed_placeholders_as_dedicated_tokens() {
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(r"f\(_, _1, _0, _name)", &mut diagnostics);
        assert!(diagnostics.is_empty());
        assert_eq!(
            tokens,
            vec![
                Token::Identifier,
                Token::SectionLParen,
                Token::Placeholder,
                Token::Comma,
                Token::Whitespace,
                Token::IndexedPlaceholder,
                Token::Comma,
                Token::Whitespace,
                Token::IndexedPlaceholder,
                Token::Comma,
                Token::Whitespace,
                Token::Identifier,
                Token::RParen,
            ]
        );
        assert_eq!(spans[2], 3..4);
        assert_eq!(spans[5], 6..8);
        assert_eq!(spans[11], 14..19);
    }

    #[test]
    fn preserves_unknown_and_unterminated_escapes_as_tokens() {
        let mut diagnostics = Vec::new();
        let (tokens, spans) = tokenize(r#""a\(b""#, &mut diagnostics);
        assert_eq!(tokens[2], Token::UnknownEscapeSequence);
        assert_eq!(spans[2], 2..4);
        assert_eq!(diagnostics.len(), 1);

        diagnostics.clear();
        let (tokens, spans) = tokenize("\"a\\", &mut diagnostics);
        assert_eq!(tokens.last(), Some(&Token::UnterminatedEscapeSequence));
        assert_eq!(spans.last(), Some(&(2..3)));
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn diagnoses_raw_string_delimiter_boundaries() {
        let too_many = format!("r{}\"value\"{}", "#".repeat(256), "#".repeat(256));
        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize(&too_many, &mut diagnostics);
        assert_eq!(tokens[0], Token::RawString);
        assert!(diagnostics[0].message.contains("255"));

        let mut diagnostics = Vec::new();
        let (tokens, _) = tokenize("r##\"unfinished\"#", &mut diagnostics);
        assert_eq!(tokens, [Token::RawString]);
        assert!(diagnostics[0].message.contains("unterminated raw String"));
    }
}
