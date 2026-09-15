//! Tree-sitter frontend. Project syntax into the flat vocabulary
//! consumed by HIR lowering; neither projection nor traversal owns recursive ASTs.
use super::{
    Token,
    cst::{CstData, Diagnostic, Node, Rule, Span},
};
use codespan_reporting::diagnostic::Label;
use tree_sitter::Node as TsNode;

mod input;
mod kinds;
#[cfg(test)]
mod tests;
mod tokens;

enum Task<'t> {
    // TSNode::parent searches from the tree root. Carry the immediate parent
    // while descending so context checks stay linear even on operator chains.
    Visit(TsNode<'t>, Option<TsNode<'t>>),
    Open(Rule),
    Close,
    Tokens(usize),
}

pub(super) fn parse(source: crate::source::SourceId, text: &str) -> crate::syntax::Parse<CstData> {
    let mut diagnostics = vec![];
    let tree = input::parse(text);
    let (tokens, spans) = tokens::extract(
        &tree,
        text.len(),
        |range| std::borrow::Cow::Borrowed(&text[range]),
        &mut diagnostics,
    );
    project(source, text.len(), tree, tokens, spans, diagnostics)
}

pub(super) fn parse_document(
    source: crate::source::SourceId,
    text: &crate::document::DocumentText,
) -> crate::syntax::Parse<CstData> {
    parse_document_cancellable(source, text, &mut || false).expect("uncancelled parse")
}

pub(super) fn parse_document_cancellable(
    source: crate::source::SourceId,
    text: &crate::document::DocumentText,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<crate::syntax::Parse<CstData>> {
    if cancelled() {
        return None;
    }
    let mut diagnostics = vec![];
    let tree = input::parse_document(text, cancelled)?;
    if cancelled() {
        return None;
    }
    let (tokens, spans) = tokens::extract_cancellable(
        &tree,
        text.byte_len(),
        |range| {
            text.slice(crate::source::TextRange::from_usize(range).expect("source range"))
                .expect("source slice")
        },
        &mut diagnostics,
        cancelled,
    )?;
    if cancelled() {
        return None;
    }
    let parsed = project_cancellable(
        source,
        text.byte_len(),
        tree,
        tokens,
        spans,
        diagnostics,
        cancelled,
    )?;
    if cancelled() { None } else { Some(parsed) }
}

fn project(
    source: crate::source::SourceId,
    byte_len: usize,
    tree: tree_sitter::Tree,
    tokens: Vec<Token>,
    spans: Vec<Span>,
    diagnostics: Vec<Diagnostic>,
) -> crate::syntax::Parse<CstData> {
    project_cancellable(
        source,
        byte_len,
        tree,
        tokens,
        spans,
        diagnostics,
        &mut || false,
    )
    .expect("uncancelled projection")
}

fn project_cancellable(
    source: crate::source::SourceId,
    byte_len: usize,
    tree: tree_sitter::Tree,
    tokens: Vec<Token>,
    spans: Vec<Span>,
    mut diagnostics: Vec<Diagnostic>,
    cancelled: &mut dyn FnMut() -> bool,
) -> Option<crate::syntax::Parse<CstData>> {
    let mut projection = Projection {
        nodes: vec![],
        open: vec![],
        tokens,
        spans,
        next: 0,
    };
    // Even an entirely damaged source owns a Program. Tree-sitter may return
    // ERROR as its root, and leading trivia must remain inside our root too.
    projection.open(Rule::Program);
    let root = tree.root_node();
    let mut root_cursor = root.walk();
    let needs_body = !root
        .children(&mut root_cursor)
        .any(|child| child.kind() == "module_body");
    if needs_body {
        projection.open(Rule::ModuleBody);
    }
    let mut tasks = vec![Task::Visit(tree.root_node(), None)];
    // Children are scheduled before the next node is visited, so one scratch
    // buffer suffices for the entire projection, regardless of tree depth.
    let mut children = Vec::new();
    let mut visited = 0usize;
    while let Some(task) = tasks.pop() {
        if visited % 256 == 0 && cancelled() {
            return None;
        }
        visited += 1;
        match task {
            Task::Open(rule) => projection.open(rule),
            Task::Close => projection.close(),
            Task::Tokens(end) => projection.tokens_until(end),
            Task::Visit(node, parent) => {
                projection.tokens_until(node.start_byte());
                if node.is_missing() {
                    let message = if node.kind() == "quote_end" {
                        "unclosed string, expected '\"'".into()
                    } else if node.kind() == ")"
                        && parent.is_some_and(|parent| parent.kind() == "paren_expr")
                    {
                        "invalid syntax, expected one of: ',', ')'".into()
                    } else {
                        format!("invalid syntax, expected {}", node.kind())
                    };
                    diagnostics.push(issue(node.byte_range(), message));
                    continue;
                }
                // Recovery can wrap one lexical error in an ERROR of the same
                // range. Let the child report it once at its precise level.
                let wraps_error = node.child_count() == 1
                    && node.child(0).is_some_and(|child| {
                        child.is_error() && child.byte_range() == node.byte_range()
                    });
                if node.is_error() && !wraps_error {
                    let message = if node.child_count() == 0 {
                        "invalid token"
                    } else {
                        "invalid syntax"
                    };
                    diagnostics.push(issue(node.byte_range(), message.into()));
                }
                if node.kind() == "invalid_function_parameter" {
                    diagnostics.push(issue(
                        node.byte_range(),
                        "invalid syntax, expected a parameter type, not a type list".into(),
                    ));
                }
                let rules = kinds::rules(node, parent);
                if rules.is_none() {
                    diagnostics.push(issue(
                        node.byte_range(),
                        format!("unsupported Tree-sitter node {}", node.kind()),
                    ));
                }
                let rules = rules.unwrap_or(&[Rule::Error]);
                for rule in rules {
                    projection.open(*rule);
                    tasks.push(Task::Close);
                }
                let mut cursor = node.walk();
                children.clear();
                children.extend(node.children(&mut cursor));
                if node.kind() == "function_contract"
                    && !children.iter().any(|child| child.kind() == "->")
                {
                    diagnostics.push(issue(
                        node.end_byte()..node.end_byte(),
                        "invalid syntax, expected '->' and a return type".into(),
                    ));
                }
                if node.kind() == "match_arm" && !children.iter().any(|child| child.kind() == "=>")
                {
                    let at = children
                        .first()
                        .map_or(node.start_byte(), |child| child.end_byte());
                    diagnostics.push(issue(
                        at..at,
                        "invalid syntax, expected one of: '=>', 'if'".into(),
                    ));
                }
                if node.kind() == "string_literal"
                    && children.iter().any(|child| child.kind() == "quote_start")
                    && !children.iter().any(|child| child.kind() == "quote_end")
                {
                    diagnostics.push(issue(
                        node.end_byte()..node.end_byte(),
                        "unclosed string, expected '\"'".into(),
                    ));
                }
                if node.kind() == "dot_postfix_expr"
                    && children.last().is_some_and(|child| child.kind() == ".")
                {
                    diagnostics.push(issue(
                        node.end_byte()..node.end_byte(),
                        "expected member after '.'".into(),
                    ));
                }
                if children.is_empty() {
                    tasks.push(Task::Tokens(node.end_byte()));
                } else if node.kind() == "contract_expr" {
                    let split = children
                        .iter()
                        .position(|n| n.kind() == "(")
                        .unwrap_or(children.len());
                    for child in children[split..].iter().rev() {
                        tasks.push(Task::Visit(*child, Some(node)));
                    }
                    tasks.push(Task::Close);
                    for child in children[..split].iter().rev() {
                        tasks.push(Task::Visit(*child, Some(node)));
                    }
                    tasks.push(Task::Open(Rule::ContractPath));
                } else if node.kind() == "block"
                    && !children.iter().any(|n| n.kind() == "block_body")
                {
                    // Empty blocks still own an empty Body in the HIR-facing vocabulary.
                    for child in children.iter().skip(1).rev() {
                        tasks.push(Task::Visit(*child, Some(node)));
                    }
                    tasks.push(Task::Close);
                    tasks.push(Task::Open(Rule::Body));
                    tasks.push(Task::Visit(children[0], Some(node)));
                } else {
                    for child in children.iter().rev().copied() {
                        tasks.push(Task::Visit(child, Some(node)));
                    }
                }
            }
        }
    }
    // Trailing trivia belongs to the root, including an entirely empty module.
    projection.tokens_until(byte_len);
    if needs_body {
        projection.close();
    }
    projection.close();
    let cst = CstData::from_projected_nodes(projection.nodes, projection.spans);
    super::finish_parse_cancellable(source, cst, diagnostics, cancelled)
}

fn issue(span: Span, message: String) -> Diagnostic {
    Diagnostic::error()
        .with_message(message)
        .with_label(Label::primary((), span))
}

struct Projection {
    nodes: Vec<Node>,
    open: Vec<(usize, Rule)>,
    tokens: Vec<Token>,
    spans: Vec<Span>,
    next: usize,
}
impl Projection {
    fn open(&mut self, rule: Rule) {
        let id = self.nodes.len();
        self.nodes.push(Node::Rule(rule, 0.into()));
        self.open.push((id, rule));
    }
    fn close(&mut self) {
        let (id, rule) = self.open.pop().expect("balanced projection");
        self.nodes[id] = Node::Rule(rule, (self.nodes.len() - id - 1).into());
    }
    fn tokens_until(&mut self, end: usize) {
        while self.next < self.tokens.len() && self.spans[self.next].end <= end {
            if self.tokens[self.next] != Token::EOF {
                self.nodes
                    .push(Node::Token(self.tokens[self.next], self.next.into()));
            }
            self.next += 1;
        }
    }
}
