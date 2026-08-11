#![allow(unused_variables)]

use super::lexer::{Token, tokenize};

// TODO: change if codespan_reporting is not used
use codespan_reporting::diagnostic::Label;
pub type Diagnostic = codespan_reporting::diagnostic::Diagnostic<()>;

#[derive(Clone, Copy)]
enum StringLookahead {
    String,
}

include!(concat!(env!("OUT_DIR"), "/telora/generated.rs"));

impl<'a> Parser<'a> {
    pub fn from_token_stream(source_len: usize, tokens: Vec<Token>, spans: Vec<Span>) -> Self {
        Self {
            current: Token::EOF,
            end_of_input: Token::EOF,
            cst: Cst {
                data: CstData::new(spans),
                source: "",
            },
            tokens,
            pos: 0,
            max_offset: source_len,
            context: (),
            error_node: None,
            in_ordered_choice: false,
            error_since_advance: false,
        }
    }
}

impl<'a> ParserCallbacks<'a> for Parser<'a> {
    type Diagnostic = Diagnostic;
    type Context = (); // TODO: add context information to the parser if required

    fn create_tokens(
        _context: &mut Self::Context,
        source: &'a str,
        diags: &mut Vec<Self::Diagnostic>,
    ) -> (Vec<Token>, Vec<Span>) {
        tokenize(source, diags)
    }
    fn create_diagnostic(&self, span: Span, message: String) -> Self::Diagnostic {
        Self::Diagnostic::error()
            .with_message(message)
            .with_label(Label::primary((), span))
    }
    fn predicate_body_1(&self) -> bool {
        matches!(
            self.current,
            Token::Let
                | Token::Decl
                | Token::Def
                | Token::Native
                | Token::Option
                | Token::Type
                | Token::Import
                | Token::Export
                | Token::At
        ) || self.current == Token::Fn && self.peek(1) == Token::Identifier
    }
    fn predicate_binding_1(&self) -> bool {
        self.current == Token::Native && self.peek(1) == Token::Type
    }
    fn predicate_binding_2(&self) -> bool {
        self.current == Token::Native && self.peek(1) != Token::Type
    }
    fn predicate_binding_3(&self) -> bool {
        self.current == Token::Let && matches!(self.peek(1), Token::LParen | Token::LBrace)
    }
    fn predicate_binding_4(&self) -> bool {
        if self.current != Token::Let
            || !matches!(self.peek(1), Token::Atom | Token::LParen | Token::LBrace)
        {
            return false;
        }
        let mut lookahead = 2usize;
        loop {
            match self.peek(lookahead) {
                Token::Else => return true,
                Token::Semicolon | Token::EOF => return false,
                _ => lookahead += 1,
            }
        }
    }
    fn predicate_binding_5(&self) -> bool {
        self.current == Token::Export
    }
    fn predicate_binding_6(&self) -> bool {
        self.current == Token::Option
    }
    fn predicate_primary_1(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_primary_2(&self) -> bool {
        self.peek(1) != Token::RBracket
    }
    fn predicate_type_arguments_1(&self) -> bool {
        self.peek(1) != Token::RBracket
    }
    fn predicate_primary_3(&self) -> bool {
        let mut depth = 0usize;
        let mut lookahead = 1usize;
        loop {
            match self.peek(lookahead) {
                Token::EOF => return false,
                Token::LParen => depth += 1,
                Token::RParen if depth == 1 => return self.peek(lookahead + 1) == Token::Arrow,
                Token::RParen => depth = depth.saturating_sub(1),
                _ => {}
            }
            lookahead += 1;
        }
    }
    fn predicate_primary_4(&self) -> bool {
        self.peek(1) == Token::Bang
    }
    fn predicate_primary_5(&self) -> bool {
        self.peek(1) == Token::Bang
    }
    fn predicate_primary_6(&self) -> bool {
        self.current == Token::If && self.peek(1) == Token::Let
    }
    fn predicate_braced_1(&self) -> bool {
        if self.peek(1) == Token::RBrace
            || self.peek(1) == Token::At
            || self.peek(1) == Token::Ellipsis
            || self.peek(1) == Token::Identifier
                && matches!(self.peek(2), Token::Colon | Token::Comma | Token::RBrace)
        {
            return true;
        }
        if self.peek(1) == Token::RawString {
            return self.peek(2) == Token::Colon;
        }
        if self.peek(1) != Token::DoubleQuote {
            return false;
        }
        let mut lookahead = 2;
        let mut contexts = vec![StringLookahead::String];
        loop {
            let token = self.peek(lookahead);
            let context = *contexts.last().expect("lookahead has a string context");
            match (context, token) {
                (_, Token::EOF) => return false,
                (StringLookahead::String, Token::DoubleQuote) => {
                    contexts.pop();
                    if contexts.is_empty() {
                        return self.peek(lookahead + 1) == Token::Colon;
                    }
                }
                _ => {}
            }
            lookahead += 1;
        }
    }
    fn predicate_dict_field_1(&self) -> bool {
        self.current == Token::Identifier && self.peek(1) != Token::Colon
    }
    fn predicate_braced_2(&self) -> bool {
        self.peek(1) != Token::RBrace
    }
    fn predicate_parameters_1(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_type_parameters_1(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_arguments_1(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_import_items_1(&self) -> bool {
        self.peek(1) != Token::RBrace
    }
    fn predicate_export_items_1(&self) -> bool {
        self.peek(1) != Token::RBrace
    }
    fn predicate_interpreter_intrinsic_1(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_named_intrinsic_1(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_pattern_2(&self) -> bool {
        self.peek(1) == Token::LParen
    }
    fn predicate_section_arguments_1(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_contract_1(&self) -> bool {
        self.current == Token::LParen
    }
    fn predicate_contract_2(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_function_contract_1(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_contract_array_1(&self) -> bool {
        self.peek(1) != Token::RBracket
    }
    fn predicate_match_expr_1(&self) -> bool {
        self.peek(1) != Token::RBrace
    }
    fn predicate_pattern_1(&self) -> bool {
        self.peek(1) != Token::RParen
    }
    fn predicate_pattern_3(&self) -> bool {
        self.peek(1) != Token::RBrace
    }
}
