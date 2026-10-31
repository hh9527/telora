//! Syntax-only source formatting. Literal bytes and the non-trivia CST are preserved.
use tree_sitter::{Node, Parser};

const INDENT: usize = 4;

/// Format a complete Telora source without resolving imports or evaluating code.
/// Width is a layout target; literals and indivisible expressions may exceed it.
pub fn format(source: &str, width: usize) -> Result<String, String> {
    if width == 0 {
        return Err("format width must be positive".into());
    }
    if source.trim().is_empty() {
        return Ok(String::new());
    }
    let mut database = crate::SourceDatabase::default();
    let id = database.add("<fmt>", source);
    let parsed = crate::syntax::telora::parse(id, source);
    if let Some(error) = parsed.diagnostics.first() {
        return Err(database.render(error));
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_telora::LANGUAGE.into())
        .map_err(|e| e.to_string())?;
    let original = parser
        .parse(source, None)
        .ok_or("source parse interrupted")?;
    if original.root_node().has_error() {
        return Err("cannot format a source with syntax errors".into());
    }
    let doc = Formatter { source }.node(original.root_node());
    let mut result = doc.render(width);
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    let formatted = parser
        .parse(&result, None)
        .ok_or("formatted parse interrupted")?;
    // Fail closed: spacing must preserve both syntax shape and exact token/literal bytes.
    if formatted.root_node().has_error()
        || fingerprint(original.root_node(), source) != fingerprint(formatted.root_node(), &result)
    {
        return Err("formatter changed the syntax structure; source was not written".into());
    }
    Ok(result)
}

fn trivia(node: Node<'_>) -> bool {
    matches!(node.kind(), "spaces" | "tabs" | "newline")
}

fn atomic(node: Node<'_>) -> bool {
    node.child_count() == 0
        || matches!(
            node.kind(),
            "string_expr"
                | "string_pattern"
                | "string_literal"
                | "raw_string"
                | "concat_string"
                | "bytes_expr"
                | "comment"
        )
}

fn children(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|node| !trivia(*node))
        .collect()
}

fn fingerprint(node: Node<'_>, source: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut stack = vec![(node, false)];
    while let Some((node, end)) = stack.pop() {
        if trivia(node) || node.kind() == "comment" {
            continue;
        }
        if end {
            result.push((")".into(), String::new()));
        } else if atomic(node) {
            result.push((node.kind().into(), source[node.byte_range()].into()));
        } else {
            result.push((node.kind().into(), "(".into()));
            stack.push((node, true));
            stack.extend(children(node).into_iter().rev().map(|child| (child, false)));
        }
    }
    // Comments can attach to a different parent after a line break, but their order
    // and contents must remain exact too.
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if node.kind() == "comment" {
            result.push(("comment".into(), source[node.byte_range()].into()));
        } else if !atomic(node) {
            stack.extend(children(node).into_iter().rev());
        }
    }
    result
}

struct Formatter<'a> {
    source: &'a str,
}

impl Formatter<'_> {
    fn text(&self, node: Node<'_>) -> &str {
        &self.source[node.byte_range()]
    }

    fn node(&self, node: Node<'_>) -> Doc {
        if trivia(node) {
            return Doc::text("");
        }
        if atomic(node) {
            return Doc::text(self.text(node));
        }
        let nodes = children(node);
        if matches!(node.kind(), "source_file" | "module_body" | "block_body") {
            return self.sequence(&nodes, node.kind(), true);
        }
        // Delimiters occur as direct children, so nested pairs cannot be confused.
        let opening = nodes
            .iter()
            .position(|n| matches!(self.text(*n), "{" | "[" | "(" | "\\("));
        if let Some(open) = opening {
            let close_text = match self.text(nodes[open]) {
                "{" => "}",
                "[" => "]",
                _ => ")",
            };
            if let Some(close) = nodes.iter().rposition(|n| self.text(*n) == close_text) {
                if close > open {
                    let mut docs = vec![];
                    if open > 0 {
                        docs.push(self.sequence(&nodes[..open], node.kind(), false));
                        docs.push(self.separator(nodes[open - 1], nodes[open], node.kind(), false));
                    }
                    docs.push(Doc::text(self.text(nodes[open])));
                    if close > open + 1 {
                        let hard = matches!(
                            node.kind(),
                            "block"
                                | "struct_initializer"
                                | "enum_initializer"
                                | "trait_binding"
                                | "impl_binding"
                                | "match_expr"
                        );
                        let boundary = if hard { Doc::hard() } else { Doc::line("") };
                        docs.push(
                            Doc::concat(vec![
                                boundary.clone(),
                                self.sequence(&nodes[open + 1..close], node.kind(), hard),
                            ])
                            .nest(),
                        );
                        docs.push(boundary);
                    }
                    docs.push(Doc::text(close_text));
                    if close + 1 < nodes.len() {
                        docs.push(self.separator(
                            nodes[close],
                            nodes[close + 1],
                            node.kind(),
                            false,
                        ));
                        docs.push(self.sequence(&nodes[close + 1..], node.kind(), false));
                    }
                    return Doc::concat(docs).group();
                }
            }
        }
        self.sequence(&nodes, node.kind(), false)
    }

    fn sequence(&self, nodes: &[Node<'_>], parent: &str, hard: bool) -> Doc {
        let mut docs = vec![];
        for (index, node) in nodes.iter().enumerate() {
            if index > 0 {
                docs.push(self.separator(nodes[index - 1], *node, parent, hard));
            }
            docs.push(self.node(*node));
        }
        Doc::concat(docs)
    }

    fn statement_break(&self, left: Node<'_>, right: Node<'_>) -> Doc {
        let gap = &self.source[left.end_byte()..right.start_byte()];
        if gap.bytes().filter(|byte| *byte == b'\n').count() >= 2 {
            Doc::concat(vec![Doc::hard(), Doc::hard()])
        } else {
            Doc::hard()
        }
    }

    fn separator(&self, left: Node<'_>, right: Node<'_>, parent: &str, hard: bool) -> Doc {
        let a = self.text(left);
        let b = self.text(right);
        if left.kind() == "comment" {
            return Doc::hard();
        }
        if right.kind() == "comment" {
            return if left.end_position().row == right.start_position().row {
                Doc::text(" ")
            } else {
                self.statement_break(left, right)
            };
        }
        if matches!(parent, "module_body" | "block_body" | "source_file") {
            return self.statement_break(left, right);
        }
        if left.kind() == "decorator" {
            return Doc::hard();
        }
        if matches!(b, "," | ";" | ":" | "." | "::" | "?" | "@")
            || b.starts_with("::")
            || matches!(a, "." | "::" | "@" | "...")
        {
            return Doc::text("");
        }
        if a == "," {
            return if hard { Doc::hard() } else { Doc::line(" ") };
        }
        if a == ";" {
            return Doc::hard();
        }
        if a == ":" {
            return Doc::text(" ");
        }
        if matches!(parent, "unary_expr" | "trait_bound") && matches!(a, "-" | "!" | "?") {
            return Doc::text("");
        }
        if (b == "!" && matches!(parent, "named_intrinsic" | "postfix_intrinsic_suffix"))
            || matches!(
                right.kind(),
                "arguments"
                    | "parameters"
                    | "type_arguments"
                    | "type_parameters"
                    | "section_arguments"
            )
            || b == "("
            || b == "\\("
            || b == "[" && matches!(parent, "index_expr" | "type_apply_expr")
        {
            return Doc::text("");
        }
        Doc::text(" ")
    }
}

#[derive(Clone)]
struct Doc {
    kind: DocKind,
    flat_width: usize,
}
#[derive(Clone)]
enum DocKind {
    Text(String),
    Line(&'static str),
    Hard,
    Concat(Vec<Doc>),
    Nest(Box<Doc>),
    Group(Box<Doc>),
}
impl Doc {
    fn text(text: &str) -> Self {
        Self {
            flat_width: if text.contains(['\r', '\n']) {
                usize::MAX
            } else {
                text.chars().count()
            },
            kind: DocKind::Text(text.into()),
        }
    }
    fn line(flat: &'static str) -> Self {
        Self {
            flat_width: flat.len(),
            kind: DocKind::Line(flat),
        }
    }
    fn hard() -> Self {
        Self {
            flat_width: usize::MAX,
            kind: DocKind::Hard,
        }
    }
    fn concat(docs: Vec<Doc>) -> Self {
        Self {
            flat_width: docs
                .iter()
                .fold(0usize, |n, d| n.saturating_add(d.flat_width)),
            kind: DocKind::Concat(docs),
        }
    }
    fn nest(self) -> Self {
        Self {
            flat_width: self.flat_width,
            kind: DocKind::Nest(Box::new(self)),
        }
    }
    fn group(self) -> Self {
        Self {
            flat_width: self.flat_width,
            kind: DocKind::Group(Box::new(self)),
        }
    }
    fn render(&self, width: usize) -> String {
        let mut output = String::new();
        let mut column = 0;
        let mut pending_indent = 0;
        let mut stack = vec![(0, false, self)];
        while let Some((indent, flat, doc)) = stack.pop() {
            match &doc.kind {
                DocKind::Text(text) => {
                    if !text.is_empty() {
                        if column == 0 {
                            output.extend(std::iter::repeat_n(' ', pending_indent));
                            column = pending_indent;
                        }
                        output.push_str(text);
                        column = if let Some((_, tail)) = text.rsplit_once('\n') {
                            tail.chars().count()
                        } else {
                            column + text.chars().count()
                        };
                    }
                }
                DocKind::Line(text) if flat => {
                    output.push_str(text);
                    column += text.len();
                }
                DocKind::Line(_) | DocKind::Hard => {
                    output.push('\n');
                    column = 0;
                    pending_indent = indent;
                }
                DocKind::Concat(docs) => stack.extend(docs.iter().rev().map(|d| (indent, flat, d))),
                DocKind::Nest(doc) => stack.push((indent + INDENT, flat, doc)),
                DocKind::Group(doc) => stack.push((
                    indent,
                    flat || doc.flat_width <= width.saturating_sub(column.max(pending_indent)),
                    doc,
                )),
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn formatted(input: &str) -> String {
        let output = format(input, 100).unwrap();
        assert_eq!(
            format(&output, 100).unwrap(),
            output,
            "formatting must be idempotent"
        );
        output
    }
    #[test]
    fn declarations_blocks_and_operators() {
        assert_eq!(
            formatted("pub def add=fn(a:Int,b:Int){let c=a+b;c};"),
            "pub def add = fn(a: Int, b: Int) {\n    let c = a + b;\n    c\n};\n"
        );
    }
    #[test]
    fn collections_paths_and_intrinsics() {
        assert_eq!(
            formatted("def v=[1,2,3];def x={a:1,b:-2};def y=std::x(v[0]).ty!(Int);"),
            "def v = [1, 2, 3];\ndef x = {a: 1, b: -2};\ndef y = std::x(v[0]).ty!(Int);\n"
        );
    }
    #[test]
    fn comments_literals_and_blank_lines() {
        let input = "# header\ndef x= r##\"a  b\n # content\"##; # trailing\n\n# second\ndef s=`value: \\{1+2}`;\n";
        let output = formatted(input);
        assert!(output.contains("r##\"a  b\n # content\"##"));
        assert!(output.contains("`value: \\{1+2}`"));
        assert!(output.contains("; # trailing\n"));
    }
    #[test]
    fn imports_and_decorators() {
        assert_eq!(
            formatted("use std::value::{ Value }; @display_by(\"x\") pub type X=struct{v:Int};"),
            "use std::value::{Value};\n@display_by(\"x\")\npub type X = struct {\n    v: Int\n};\n"
        );
    }
    #[test]
    fn narrow_collections_wrap() {
        let output = format("def x = [1111, 2222, 3333, 4444];", 20).unwrap();
        assert!(output.contains("[\n    1111,\n"), "{output}");
        assert_eq!(format(&output, 20).unwrap(), output);
    }
    #[test]
    fn invalid_or_incomplete_source_is_rejected() {
        for input in ["def x = [;", "let = ;", "def x = \"unterminated"] {
            assert!(format(input, 100).is_err(), "{input}");
        }
    }
    #[test]
    fn empty_source_and_crlf() {
        assert_eq!(formatted(" \r\n\t"), "");
        assert_eq!(
            formatted("def x=1;\r\ndef y=2;\r\n"),
            "def x = 1;\ndef y = 2;\n"
        );
    }
    #[test]
    fn formats_repository_syntax_without_changing_programs() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut pending = vec![
            root.join("modules"),
            root.join("../../tree-sitter-telora/test"),
            root.join("../../tests"),
            root.join("../../examples"),
        ];
        let mut checked = 0;
        while let Some(path) = pending.pop() {
            if path.is_dir() {
                pending.extend(
                    std::fs::read_dir(path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
            } else if path.extension().is_some_and(|ext| ext == "telora") {
                let source = std::fs::read_to_string(&path).unwrap();
                let mut db = crate::SourceDatabase::default();
                let id = db.add("<fixture>", &source);
                if !crate::syntax::telora::parse(id, &source)
                    .diagnostics
                    .is_empty()
                {
                    continue;
                }
                let output = format(&source, 100)
                    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
                assert_eq!(
                    format(&output, 100).unwrap(),
                    output,
                    "{} is not idempotent",
                    path.display()
                );
                checked += 1;
            }
        }
        assert!(checked > 50, "only checked {checked} sources");
        eprintln!("formatted and checked {checked} repository sources");
    }
}
