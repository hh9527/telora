use super::*;

impl Lower<'_> {
    pub(super) fn string(&self, node: NodeRef) -> Result<Shape, ()> {
        let text = self
            .cst
            .children(node)
            .find(|child| {
                matches!(
                    self.rule(*child),
                    Some(Rule::StringLiteral | Rule::ConcatExpression)
                )
            })
            .ok_or(())?;
        if self.rule(text) == Some(Rule::StringLiteral) {
            return Ok(Shape::Node(
                HirKind::String(self.plain_string(text)?),
                vec![],
            ));
        }
        let mut parts = vec![];
        for component in self.string_components(text) {
            if self.rule(component) == Some(Rule::Interpolation) {
                self.needs_display.set(true);
                let expression = self.first_expression(component)?;
                let receiver = self.synthetic(
                    Role::Receiver,
                    expression,
                    HirKind::Variable("\0interpolation_display".into()),
                    vec![],
                );
                let name = self.synthetic(
                    Role::Name,
                    expression,
                    HirKind::Name("display".into()),
                    vec![],
                );
                let callee = self.synthetic(
                    Role::Callee,
                    expression,
                    HirKind::Field,
                    vec![receiver, name],
                );
                parts.push(self.synthetic(
                    Role::Part,
                    expression,
                    HirKind::Call,
                    vec![callee, Input::expr(Role::Argument, expression)],
                ));
            } else {
                parts.push(self.synthetic(
                    Role::Part,
                    component,
                    HirKind::String(self.string_component(component)?),
                    vec![],
                ));
            }
        }
        Ok(Shape::Node(HirKind::InterpolatedString, parts))
    }

    pub(super) fn plain_string(&self, node: NodeRef) -> Result<String, ()> {
        let mut text = String::new();
        for component in self.string_components(node) {
            text.push_str(&self.string_component(component)?);
        }
        Ok(text)
    }

    fn string_components(&self, node: NodeRef) -> Vec<NodeRef> {
        let mut output = vec![];
        let mut cursor = node.0;
        let end = match self.cst.get(node) {
            Node::Rule(_, offset) => node.0 + usize::from(offset),
            _ => node.0,
        };
        while cursor <= end {
            let current = NodeRef(cursor);
            match self.cst.get(current) {
                Node::Rule(Rule::Interpolation, offset) => {
                    output.push(current);
                    cursor += usize::from(offset);
                }
                Node::Token(Token::StringText | Token::EscapeSequence | Token::RawString, _) => {
                    output.push(current)
                }
                _ => {}
            }
            cursor += 1;
        }
        output
    }

    fn string_component(&self, node: NodeRef) -> Result<String, ()> {
        let text = self.text(node);
        match self.cst.get(node) {
            Node::Token(Token::StringText, _) => Ok(normalize_newlines(&text)),
            Node::Token(Token::RawString, _) => {
                let hashes = text[1..].bytes().take_while(|byte| *byte == b'#').count();
                if hashes > 255 {
                    return Err(self.error(node, "raw String delimiter exceeds 255 # characters"));
                }
                let opener = hashes + 2;
                let terminator = format!("\"{}", "#".repeat(hashes));
                if text.len() < opener + terminator.len() || !text.ends_with(&terminator) {
                    return Err(self.error(node, "unterminated raw String"));
                }
                Ok(normalize_newlines(
                    &text[opener..text.len() - terminator.len()],
                ))
            }
            Node::Token(Token::EscapeSequence, _) => {
                let escaped = &text[1..];
                if !escaped.is_empty() && escaped.chars().all(char::is_whitespace) {
                    return Ok(String::new());
                }
                Ok(match escaped {
                    "0" => "\0".into(),
                    "n" => "\n".into(),
                    "r" => "\r".into(),
                    "t" => "\t".into(),
                    "\"" => "\"".into(),
                    "`" => "`".into(),
                    "\\" => "\\".into(),
                    value if value.starts_with('x') => {
                        let byte = u8::from_str_radix(&value[1..], 16)
                            .map_err(|_| self.error(node, "invalid ASCII string escape"))?;
                        if !byte.is_ascii() {
                            return Err(self.error(node, "\\x string escape must be ASCII"));
                        }
                        char::from(byte).to_string()
                    }
                    value if value.starts_with("u{") && value.ends_with('}') => {
                        let scalar = u32::from_str_radix(&value[2..value.len() - 1], 16)
                            .ok()
                            .and_then(char::from_u32)
                            .ok_or_else(|| self.error(node, "invalid Unicode scalar escape"))?;
                        scalar.to_string()
                    }
                    _ => return Err(self.error(node, format!("unsupported escape \\{escaped}"))),
                })
            }
            _ => Err(self.error(node, "expected string text or escape")),
        }
    }

    pub(super) fn bytes(&self, node: NodeRef) -> Result<Shape, ()> {
        let token = self.required_token(node, Token::Bytes)?;
        let text = normalize_newlines(&self.text(token));
        let quoted = text.strip_prefix('b').unwrap_or(&text);
        let mut chars = quoted[1..quoted.len() - 1].chars();
        let mut output = String::new();
        while let Some(character) = chars.next() {
            if character != '\\' {
                output.push(character);
                continue;
            }
            output.push(match chars.next() {
                Some('n') => '\n',
                Some('r') => '\r',
                Some('t') => '\t',
                Some('"') => '"',
                Some('\\') => '\\',
                Some(other) => {
                    return Err(self.error(token, format!("unsupported escape \\{other}")));
                }
                None => return Err(self.error(token, "unterminated string escape")),
            });
        }
        Ok(Shape::Node(HirKind::Bytes(output.into_bytes()), vec![]))
    }
}

fn normalize_newlines(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_owned()
    }
}
