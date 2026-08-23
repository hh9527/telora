use super::*;
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

include!("part-01.rs");
