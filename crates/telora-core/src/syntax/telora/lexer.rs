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
    Do,
    Native,
    Option,
    For,
    Type,
    Trait,
    Impl,
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
    Percent,
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
    StructInitializer,
    EnumInitializer,
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
    #[token("do")]
    Do,
    #[token("native")]
    Native,
    #[token("option")]
    Option,
    #[token("for")]
    For,
    #[token("type")]
    Type,
    #[token("trait")]
    Trait,
    #[token("impl")]
    Impl,
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
    #[token("%")]
    Percent,
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
        contextualize_declared_type_tokens(&mut tokens, |index| {
            match &source[spans[index].clone()] {
                "struct" => Some(Token::StructInitializer),
                "enum" => Some(Token::EnumInitializer),
                _ => None,
            }
        });
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

fn contextualize_declared_type_tokens(
    tokens: &mut [Token],
    mut classify: impl FnMut(usize) -> Option<Token>,
) {
    let significant = tokens
        .iter()
        .enumerate()
        .filter_map(|(index, token)| {
            (!matches!(token, Token::Whitespace | Token::Comment)).then_some(index)
        })
        .collect::<Vec<_>>();
    let mut cursor = 0usize;
    while cursor < significant.len() {
        if tokens[significant[cursor]] != Token::Type {
            cursor += 1;
            continue;
        }
        let Some(&name) = significant.get(cursor + 1) else {
            break;
        };
        if tokens[name] != Token::Identifier {
            cursor += 1;
            continue;
        }
        let mut next = cursor + 2;
        if significant
            .get(next)
            .is_some_and(|index| tokens[*index] == Token::LParen)
        {
            next += 1;
            while significant
                .get(next)
                .is_some_and(|index| tokens[*index] != Token::RParen)
            {
                next += 1;
            }
            if significant.get(next).is_none() {
                break;
            }
            next += 1;
        }
        let (Some(&equal), Some(&initializer), Some(&brace)) = (
            significant.get(next),
            significant.get(next + 1),
            significant.get(next + 2),
        ) else {
            break;
        };
        if tokens[equal] == Token::Equal
            && tokens[initializer] == Token::Identifier
            && tokens[brace] == Token::LBrace
            && let Some(kind) = classify(initializer)
        {
            tokens[initializer] = kind;
        }
        cursor += 1;
    }
}

pub fn tokenize_document(
    source: &crate::document::DocumentText,
    diags: &mut Vec<Diagnostic>,
) -> (Vec<Token>, Vec<Span>) {
    tokenize_fragments(source, source.chunks(), diags)
}

fn tokenize_fragments<'a>(
    source: &crate::document::DocumentText,
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
    contextualize_declared_type_tokens(&mut tokens, |index| {
        let range = crate::source::TextRange::from_usize(spans[index].clone())
            .expect("lexer span fits document");
        match source
            .slice(range)
            .expect("lexer span is a document slice")
            .as_ref()
        {
            "struct" => Some(Token::StructInitializer),
            "enum" => Some(Token::EnumInitializer),
            _ => None,
        }
    });
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
            NormalToken::Do => Self::Do,
            NormalToken::Native => Self::Native,
            NormalToken::Option => Self::Option,
            NormalToken::For => Self::For,
            NormalToken::Type => Self::Type,
            NormalToken::Trait => Self::Trait,
            NormalToken::Impl => Self::Impl,
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
            NormalToken::Percent => Self::Percent,
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
#[path = "lexer/tests/mod.rs"]
mod tests;
