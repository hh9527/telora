use super::*;

impl Parser<'_> {
    fn operand_depth(&self) -> u32 {
        self.context
            .token_depths
            .get(self.pos)
            .copied()
            .unwrap_or(0)
    }

    pub(super) fn begin_operand(&mut self) {
        self.context.operands.push(self.operand_depth());
    }

    pub(super) fn end_operand(&mut self) {
        self.context
            .operands
            .pop()
            .expect("balanced grammar operand actions");
    }

    pub(super) fn check_control_operand(&mut self, diags: &mut Vec<Diagnostic>) {
        if self.current == Token::EOF
            || !self
                .context
                .operands
                .last()
                .is_some_and(|&boundary| self.operand_depth() <= boundary)
        {
            return;
        }
        diags.push(
            Diagnostic::error()
                .with_message("nested control expression requires parentheses")
                .with_label(Label::primary((), self.span())),
        );
        // This module has a syntax error. Consume the remainder without
        // descending into the rejected expression; keep its tokens losslessly.
        self.error_node = Some(self.open(diags));
        while self.current != Token::EOF {
            self.advance(true, diags);
        }
    }
}
