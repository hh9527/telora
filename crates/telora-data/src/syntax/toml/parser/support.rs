use super::*;
use alloc::{string::String, vec::Vec};

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
    type Context = ();

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
}
