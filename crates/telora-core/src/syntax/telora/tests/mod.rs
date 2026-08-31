use super::*;
use ast::{AstNode, Binding, ExpectedSyntax, Program, StringLiteral};
use lexer::Token;
use parser::{Node, NodeRef};

fn reconstruct(cst: &CstData, source: &str, node: NodeRef, output: &mut String) {
    match cst.get(node) {
        Node::Token(..) => output.push_str(&source[cst.span(node)]),
        Node::Rule(..) => {
            for child in cst.children(node) {
                reconstruct(cst, source, child, output);
            }
        }
    }
}

fn find_rule(cst: &CstData, node: NodeRef, expected: parser::Rule) -> Option<NodeRef> {
    if matches!(cst.get(node), Node::Rule(rule, _) if rule == expected) {
        return Some(node);
    }
    cst.children(node)
        .find_map(|child| find_rule(cst, child, expected))
}

fn contains_rule_error(cst: &CstData, node: NodeRef) -> bool {
    matches!(cst.get(node), Node::Rule(parser::Rule::Error, _))
        || cst
            .children(node)
            .any(|child| contains_rule_error(cst, child))
}

include!("part-01.rs");
