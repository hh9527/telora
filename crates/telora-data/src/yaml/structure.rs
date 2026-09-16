//! Flat parse-0 graph: source ranges and folding instructions, no decoded data.
use crate::{
    json::{DataField, DataNodeId},
    source::{Diagnostic, Location},
};
use alloc::vec::Vec;
use core::ops::Range;

#[derive(Debug)]
pub(super) enum Piece {
    Source(Range<usize>),
    Spaces(usize),
    Newlines(usize),
}
#[derive(Debug)]
pub(super) enum Text {
    Source(Range<usize>),
    Quoted(Range<usize>),
    Block(Range<usize>),
}
#[derive(Debug)]
pub(super) enum Scalar {
    Null,
    Bool(bool),
    Number { range: Range<usize>, float: bool },
    String(Text),
    Bytes(Range<usize>),
}
#[derive(Debug)]
pub(super) enum Kind {
    Scalar(Scalar),
    Array(Vec<DataNodeId>),
    Object(Vec<(Text, DataField)>),
}
#[derive(Debug)]
pub(super) struct Node {
    pub kind: Kind,
    pub location: Location,
    // Match resolved numeric alignment on wasm32 too, without storing a payload.
    _numeric_alignment: [f64; 0],
}
#[derive(Debug, Default)]
pub(super) struct Plan {
    pub nodes: Vec<Node>,
    pub root: Option<DataNodeId>,
    pub diagnostics: Vec<Diagnostic>,
    pub pieces: Vec<Piece>,
}
impl Plan {
    fn push(&mut self, kind: Kind, location: Location) -> DataNodeId {
        let id = DataNodeId(self.nodes.len());
        self.nodes.push(Node {
            kind,
            location,
            _numeric_alignment: [],
        });
        id
    }
    pub fn scalar(&mut self, value: Scalar, loc: Location) -> DataNodeId {
        self.push(Kind::Scalar(value), loc)
    }
    pub fn array(&mut self, items: Vec<DataNodeId>, loc: Location) -> DataNodeId {
        self.push(Kind::Array(items), loc)
    }
    pub fn object(&mut self, fields: Vec<(Text, DataField)>, loc: Location) -> DataNodeId {
        self.push(Kind::Object(fields), loc)
    }
    pub fn node(&self, id: DataNodeId) -> &Node {
        &self.nodes[id.index()]
    }
    pub fn set_root(&mut self, root: DataNodeId) {
        self.root = Some(root);
    }
}

const _: [(); core::mem::size_of::<Node>()] = [(); core::mem::size_of::<super::YamlNode>()];
const _: [(); core::mem::align_of::<Node>()] = [(); core::mem::align_of::<super::YamlNode>()];
const _: [(); core::mem::size_of::<(Text, DataField)>()] =
    [(); core::mem::size_of::<(crate::json::text::TextSpan, DataField)>()];
const _: [(); core::mem::align_of::<(Text, DataField)>()] =
    [(); core::mem::align_of::<(crate::json::text::TextSpan, DataField)>()];
