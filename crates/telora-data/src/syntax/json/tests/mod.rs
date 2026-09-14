use super::*;
use alloc::string::String;
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

mod cases;
