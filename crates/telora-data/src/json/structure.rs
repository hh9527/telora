//! Parse-0 output: source spans and a flat graph, without decoded payloads.
use super::DataNodeId;
use crate::source::Location;
use alloc::vec::Vec;
use core::ops::Range;

#[derive(Clone, Debug)]
pub(crate) struct StringSpan {
    pub range: Range<usize>,
    pub escaped: bool,
}

#[derive(Debug)]
pub(crate) enum RawKind {
    String(StringSpan),
    Number { range: Range<usize>, float: bool },
    Null,
    Bool(bool),
    Array(Vec<DataNodeId>),
    Object(Vec<(StringSpan, super::DataField)>),
}

#[derive(Debug)]
pub(crate) struct RawNode {
    pub kind: RawKind,
    pub location: Location,
    // No payload: reserve the target alignment needed by resolved numbers.
    _numeric_alignment: [f64; 0],
}

#[derive(Debug, Default)]
pub(crate) struct RawPlan {
    pub nodes: Vec<RawNode>,
    pub root: Option<DataNodeId>,
    pub diagnostics: Vec<crate::source::Diagnostic>,
}

impl RawPlan {
    pub fn push(&mut self, kind: RawKind, location: Location) -> DataNodeId {
        let id = DataNodeId(self.nodes.len());
        self.nodes.push(RawNode {
            kind,
            location,
            _numeric_alignment: [],
        });
        id
    }
}

#[derive(Clone, Debug)]
pub enum JsonKind {
    Int(i64),
    Float(f64),
    String(super::text::TextSpan),
    Null,
    Bool(bool),
    Array(Vec<DataNodeId>),
    /// Sorted by resolved key contents, never by span offsets.
    Object(Vec<(super::text::TextSpan, super::DataField)>),
}

#[derive(Clone, Debug)]
pub struct JsonNode {
    pub kind: JsonKind,
    pub location: Location,
}

#[derive(Clone, Debug)]
pub struct JsonPlan {
    pub nodes: Vec<JsonNode>,
    pub root: DataNodeId,
}

// Vec's in-place conversion requires the same slot and field layout on both
// the Host and wasm32; do not accidentally reintroduce a second arena there.
const _: [(); core::mem::size_of::<RawNode>()] = [(); core::mem::size_of::<JsonNode>()];
const _: [(); core::mem::align_of::<RawNode>()] = [(); core::mem::align_of::<JsonNode>()];
const _: [(); core::mem::size_of::<(StringSpan, super::DataField)>()] =
    [(); core::mem::size_of::<(super::text::TextSpan, super::DataField)>()];
const _: [(); core::mem::align_of::<(StringSpan, super::DataField)>()] =
    [(); core::mem::align_of::<(super::text::TextSpan, super::DataField)>()];
