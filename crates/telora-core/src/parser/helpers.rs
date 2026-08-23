impl<'a> Lowerer<'a> {
    fn expression_children(&self, node: NodeRef) -> Result<Vec<Expr>, Diagnostic> {
        self.children(node)
            .filter(|child| self.is_expression(*child))
            .map(|child| self.expression(child))
            .collect()
    }

    fn type_arguments(&self, node: NodeRef) -> Result<Vec<TypeArgument>, Diagnostic> {
        self.rule_children(node)
            .filter(|child| self.rule(*child) == Some(Rule::TypeArgument))
            .map(|argument| {
                if let Some(placeholder) = self.token_children(argument, Token::Placeholder).next()
                {
                    return Ok(located(TypeArgumentKind::Infer, self.location(placeholder)));
                }
                let expression = self
                    .children(argument)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(argument, "type argument has no expression"))?;
                let expression = self.expression(expression)?;
                let location = expression.location;
                Ok(located(TypeArgumentKind::Explicit(expression), location))
            })
            .collect()
    }

    fn section_expression(
        &self,
        callee: Expr,
        arguments_node: Option<NodeRef>,
        section_node: NodeRef,
        location: Location,
    ) -> Result<Expr, Diagnostic> {
        let Some(arguments_node) = arguments_node else {
            return Err(self.error(section_node, "call section has no arguments"));
        };
        let arguments = self
            .rule_children(arguments_node)
            .filter(|child| self.rule(*child) == Some(Rule::Argument))
            .map(|argument| self.call_argument(argument))
            .collect::<Result<Vec<_>, _>>()?;
        elaborate_call_section(callee, arguments, section_node, location)
            .map_err(|(node, message)| self.error(node, message))
    }

    fn call_argument(&self, node: NodeRef) -> Result<CallArgument, Diagnostic> {
        if let Some(placeholder) = self.token_children(node, Token::Placeholder).next() {
            return Ok(CallArgument::Bare {
                node: placeholder,
                location: self.location(placeholder),
            });
        }
        if let Some(placeholder) = self.token_children(node, Token::IndexedPlaceholder).next() {
            let text = self.text(placeholder);
            let index = text[1..].parse::<usize>().map_err(|_| {
                self.error(placeholder, "placeholder index exceeds the supported range")
            })?;
            return Ok(CallArgument::Indexed {
                node: placeholder,
                index,
                location: self.location(placeholder),
            });
        }
        let expression = self
            .children(node)
            .find(|child| self.is_expression(*child))
            .ok_or_else(|| self.error(node, "call argument has no expression"))?;
        Ok(CallArgument::Expression(self.expression(expression)?))
    }
    fn is_expression(&self, node: NodeRef) -> bool {
        matches!(
            self.cst.get(node),
            Node::Token(
                Token::Int | Token::Float | Token::Bytes | Token::Atom | Token::Identifier,
                _
            )
        ) || matches!(
            self.rule(node),
            Some(
                Rule::Expression
                    | Rule::Primary
                    | Rule::Braced
                    | Rule::ArrayExpr
                    | Rule::AtomExpr
                    | Rule::BinaryExpr
                    | Rule::Block
                    | Rule::BytesExpr
                    | Rule::CallExpr
                    | Rule::Closure
                    | Rule::DictExpr
                    | Rule::DoExpr
                    | Rule::IndexExpr
                    | Rule::FloatExpr
                    | Rule::FunctionContract
                    | Rule::IfExpr
                    | Rule::IfLetExpr
                    | Rule::InterpreterIntrinsic
                    | Rule::NamedIntrinsic
                    | Rule::LegacyInterpreterExpr
                    | Rule::IntExpr
                    | Rule::MatchExpr
                    | Rule::ParenExpr
                    | Rule::PipelineExpr
                    | Rule::DotPostfixExpr
                    | Rule::PropagateExpr
                    | Rule::ReturnExpr
                    | Rule::SectionExpr
                    | Rule::SpreadExpr
                    | Rule::StringExpr
                    | Rule::TypeApplyExpr
                    | Rule::UnaryExpr
                    | Rule::VariableExpr
            )
        )
    }
    fn is_pattern(&self, node: NodeRef) -> bool {
        matches!(
            self.cst.get(node),
            Node::Token(
                Token::Identifier | Token::Placeholder | Token::Int | Token::Float | Token::Atom,
                _
            )
        ) || matches!(
            self.rule(node),
            Some(
                Rule::Pattern
                    | Rule::AtomPattern
                    | Rule::FloatPattern
                    | Rule::IdentifierPattern
                    | Rule::IntPattern
                    | Rule::StringPattern
                    | Rule::TaggedPattern
                    | Rule::TuplePattern
                    | Rule::StructPattern
            )
        )
    }
    fn children(&self, node: NodeRef) -> impl Iterator<Item = NodeRef> + '_ {
        self.cst.children(node)
    }
    fn rule_children(&self, node: NodeRef) -> impl Iterator<Item = NodeRef> + '_ {
        self.children(node)
            .filter(|child| matches!(self.cst.get(*child), Node::Rule(..)))
    }
    fn token_children(&self, node: NodeRef, token: Token) -> impl Iterator<Item = NodeRef> + '_ {
        self.children(node).filter(
            move |child| matches!(self.cst.get(*child), Node::Token(found, _) if found == token),
        )
    }
    fn first_rule(&self, node: NodeRef) -> Option<NodeRef> {
        self.rule_children(node).next()
    }
    fn expression_head(&self, mut node: NodeRef) -> NodeRef {
        while matches!(
            self.rule(node),
            Some(Rule::Expression | Rule::Primary | Rule::Braced)
        ) {
            let Some(child) = self.first_rule(node) else {
                break;
            };
            node = child;
        }
        node
    }
    fn first_token(&self, node: NodeRef, token: Token) -> Result<NodeRef, Diagnostic> {
        self.token_children(node, token)
            .next()
            .ok_or_else(|| self.error(node, format!("missing {token:?}")))
    }
    fn rule(&self, node: NodeRef) -> Option<Rule> {
        match self.cst.get(node) {
            Node::Rule(rule, _) => Some(rule),
            Node::Token(..) => None,
        }
    }
    fn location(&self, node: NodeRef) -> Location {
        Location::from_usize(self.source_id, self.cst.span(node))
            .expect("CST span fits registered source")
    }
    fn identifier(&self, node: NodeRef) -> Identifier {
        located(self.text(node).into_owned(), self.location(node))
    }
    fn text(&self, node: NodeRef) -> std::borrow::Cow<'_, str> {
        self.source
            .slice(
                crate::source::TextRange::from_usize(self.cst.span(node))
                    .expect("CST span fits registered source"),
            )
            .expect("CST span is a valid source slice")
    }
    fn error(&self, node: NodeRef, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.location(node))
    }

    fn string_expression(&self, node: NodeRef) -> Result<Expr, Diagnostic> {
        let text_node = self
            .rule_children(node)
            .find(|child| {
                matches!(
                    self.rule(*child),
                    Some(Rule::StringLiteral | Rule::ConcatExpression)
                )
            })
            .ok_or_else(|| self.error(node, "string expression has no text"))?;
        let mut components = Vec::new();
        self.collect_string_components(text_node, &mut components);
        if self.rule(text_node) == Some(Rule::StringLiteral) {
            return Ok(located(
                ExprKind::String(self.decode_string_components(&components)?),
                self.location(node),
            ));
        }

        let mut parts = Vec::new();
        for component in components {
            if self.rule(component) == Some(Rule::Interpolation) {
                let expression_node = self
                    .children(component)
                    .find(|child| self.is_expression(*child))
                    .ok_or_else(|| self.error(component, "interpolation has no expression"))?;
                let expression = self.expression(expression_node)?;
                let location = expression.location;
                parts.push(located(StringPartKind::Expression(expression), location));
            } else {
                parts.push(located(
                    StringPartKind::Text(self.decode_string_component(component)?),
                    self.location(component),
                ));
            }
        }
        Ok(located(
            ExprKind::InterpolatedString(parts),
            self.location(node),
        ))
    }

    fn plain_string(&self, node: NodeRef, context: &str) -> Result<String, Diagnostic> {
        let mut components = Vec::new();
        self.collect_string_components(node, &mut components);
        let _ = context;
        self.decode_string_components(&components)
    }

    fn collect_string_components(&self, node: NodeRef, output: &mut Vec<NodeRef>) {
        match self.cst.get(node) {
            Node::Token(Token::StringText | Token::EscapeSequence | Token::RawString, _) => {
                output.push(node)
            }
            Node::Rule(Rule::Interpolation, _) => output.push(node),
            Node::Token(..) => {}
            Node::Rule(..) => {
                for child in self.children(node) {
                    self.collect_string_components(child, output);
                }
            }
        }
    }

    fn decode_string_components(&self, components: &[NodeRef]) -> Result<String, Diagnostic> {
        let mut output = String::new();
        for component in components {
            output.push_str(&self.decode_string_component(*component)?);
        }
        Ok(output)
    }

    fn decode_string_component(&self, node: NodeRef) -> Result<String, Diagnostic> {
        match self.cst.get(node) {
            Node::Token(Token::StringText, _) => Ok(self.text(node).into_owned()),
            Node::Token(Token::EscapeSequence, _) => self.decode_escape(node),
            Node::Token(Token::RawString, _) => self.decode_raw_string(node),
            _ => Err(self.error(node, "expected string text or escape")),
        }
    }

    fn decode_escape(&self, node: NodeRef) -> Result<String, Diagnostic> {
        let text = self.text(node);
        let escaped = &text[1..];
        if escaped.starts_with(['\n', '\r']) {
            return Ok(String::new());
        }
        let decoded = match escaped {
            "0" => "\0".to_owned(),
            "n" => "\n".to_owned(),
            "r" => "\r".to_owned(),
            "t" => "\t".to_owned(),
            "\"" => "\"".to_owned(),
            "`" => "`".to_owned(),
            "\\" => "\\".to_owned(),
            value if value.starts_with('x') => {
                let byte = u8::from_str_radix(&value[1..], 16)
                    .map_err(|_| self.error(node, "invalid ASCII string escape"))?;
                if !byte.is_ascii() {
                    return Err(self.error(node, "\\x string escape must be ASCII"));
                }
                char::from(byte).to_string()
            }
            value if value.starts_with("u{") => {
                let digits = &value[2..value.len() - 1];
                let scalar = u32::from_str_radix(digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| self.error(node, "invalid Unicode scalar escape"))?;
                scalar.to_string()
            }
            _ => return Err(self.error(node, format!("unsupported escape \\{escaped}"))),
        };
        Ok(decoded)
    }

    fn decode_raw_string(&self, node: NodeRef) -> Result<String, Diagnostic> {
        let text = self.text(node);
        let hashes = text[1..].bytes().take_while(|byte| *byte == b'#').count();
        if hashes > 255 {
            return Err(self.error(node, "raw String delimiter exceeds 255 # characters"));
        }
        let opener = hashes + 2;
        let terminator = format!("\"{}", "#".repeat(hashes));
        if text.len() < opener + terminator.len() || !text.ends_with(&terminator) {
            return Err(self.error(node, "unterminated raw String"));
        }
        Ok(text[opener..text.len() - terminator.len()].to_owned())
    }

    fn decode_telora_string(&self, node: NodeRef) -> Result<String, Diagnostic> {
        let text = self.text(node);
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
                    return Err(self.error(node, format!("unsupported escape \\{other}")));
                }
                None => return Err(self.error(node, "unterminated string escape")),
            });
        }
        Ok(output)
    }
}

