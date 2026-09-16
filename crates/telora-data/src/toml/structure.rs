//! Parse-0 preserves table declaration blocks, not resolved data tables.
use crate::source::{Diagnostic, Location};
use alloc::vec::Vec;
use core::ops::Range;

#[derive(Clone, Debug)]
pub(super) struct Text {
    pub range: Range<usize>,
    pub decoded_len: usize,
    pub escaped: bool,
}
#[derive(Debug)]
pub(super) struct Key {
    pub text: Text,
    pub location: Location,
}
#[derive(Debug)]
pub(super) enum Scalar {
    Bool(bool),
    String(Text),
    Atom(Range<usize>),
}
#[derive(Debug)]
pub(super) enum Kind {
    Scalar(Scalar),
    Array(Vec<usize>),
    Inline(Vec<Assignment>),
}
#[derive(Debug)]
pub(super) struct Node {
    pub kind: Kind,
    pub location: Location,
}
#[derive(Debug)]
pub(super) struct Assignment {
    pub path: Vec<usize>,
    pub value: usize,
}
#[derive(Debug)]
pub(super) struct Header {
    pub path: Vec<usize>,
    pub array: bool,
    pub location: Location,
}
#[derive(Debug)]
pub(super) struct Table {
    pub header: Option<Header>,
    pub items: Vec<Assignment>,
}
#[derive(Debug, Default)]
pub(super) struct Plan {
    pub nodes: Vec<Node>,
    pub keys: Vec<Key>,
    pub tables: Vec<Table>,
    pub diagnostics: Vec<Diagnostic>,
}
impl Plan {
    pub fn push(&mut self, kind: Kind, location: Location) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Node { kind, location });
        id
    }
}
