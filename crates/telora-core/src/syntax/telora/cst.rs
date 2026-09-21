//! Flat semantic CST: stable node indices and borrowed traversal, without parser state.
use super::Token;
mod rule;
pub use rule::Rule;
pub type Diagnostic = codespan_reporting::diagnostic::Diagnostic<()>;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct NodeRef(pub usize);

impl NodeRef {
    #[allow(dead_code)]
    pub const ROOT: NodeRef = NodeRef(0);
}

#[cfg(target_pointer_width = "64")]
#[derive(Copy, Clone)]
pub struct CstIndex([u8; 6]);

#[cfg(any(target_pointer_width = "16", target_pointer_width = "32"))]
#[derive(Copy, Clone)]
pub struct CstIndex(usize);

impl std::fmt::Debug for CstIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        usize::from(*self).fmt(f)
    }
}

impl From<CstIndex> for usize {
    #[cfg(target_pointer_width = "64")]
    #[inline]
    fn from(value: CstIndex) -> Self {
        let [b0, b1, b2, b3, b4, b5] = value.0;
        usize::from_le_bytes([b0, b1, b2, b3, b4, b5, 0, 0])
    }
    #[cfg(any(target_pointer_width = "16", target_pointer_width = "32"))]
    #[inline]
    fn from(value: CstIndex) -> Self {
        value.0
    }
}
impl From<usize> for CstIndex {
    #[cfg(target_pointer_width = "64")]
    #[inline]
    fn from(value: usize) -> Self {
        let [b0, b1, b2, b3, b4, b5, b6, b7] = value.to_le_bytes();
        debug_assert!(b6 == 0 && b7 == 0);
        Self([b0, b1, b2, b3, b4, b5])
    }
    #[cfg(any(target_pointer_width = "16", target_pointer_width = "32"))]
    #[inline]
    fn from(value: usize) -> Self {
        Self(value)
    }
}

/// Type of a node in the CST.
///
/// The nodes for rules contain the offset to their last child node.
/// The nodes for tokens contain an index to their span.
///
/// On 64 bit platforms offsets and indices are stored as 48 bit integers.
/// This allows the `Node` type to be 8 bytes in size as long as the `Rule`
/// and `Token` enums are one byte in size.
#[derive(Debug, Copy, Clone)]
pub enum Node {
    Rule(Rule, CstIndex),
    Token(Token, CstIndex),
}

/// An iterator for child nodes of a CST node.
#[derive(Default)]
pub struct CstChildren<'a> {
    iter: std::slice::Iter<'a, Node>,
    offset: usize,
}
impl Iterator for CstChildren<'_> {
    type Item = NodeRef;

    fn next(&mut self) -> Option<Self::Item> {
        let offset = self.offset;
        self.offset += 1;
        if let Some(node) = self.iter.next() {
            if let Node::Rule(_, end_offset) = node {
                let end_offset = usize::from(*end_offset);
                if end_offset > 0 {
                    self.iter.nth(end_offset.saturating_sub(1));
                    self.offset += end_offset;
                }
            }
            Some(NodeRef(offset))
        } else {
            None
        }
    }
}

pub type Span = core::ops::Range<usize>;

#[derive(Debug)]
pub struct CstData {
    spans: Vec<Span>,
    nodes: Vec<Node>,
}
impl CstData {
    pub(crate) fn from_projected_nodes(nodes: Vec<Node>, spans: Vec<Span>) -> Self {
        Self { nodes, spans }
    }
    pub fn children(&self, node_ref: NodeRef) -> CstChildren<'_> {
        let iter = if let Node::Rule(_, end_offset) = self.nodes[node_ref.0] {
            self.nodes[node_ref.0 + 1..node_ref.0 + usize::from(end_offset) + 1].iter()
        } else {
            std::slice::Iter::default()
        };
        CstChildren {
            iter,
            offset: node_ref.0 + 1,
        }
    }
    pub fn get(&self, node_ref: NodeRef) -> Node {
        self.nodes[node_ref.0]
    }
    pub fn span(&self, node_ref: NodeRef) -> Span {
        fn find_token<'a>(mut iter: impl Iterator<Item = &'a Node>) -> Option<usize> {
            iter.find_map(|node| match node {
                Node::Rule(..) => None,
                Node::Token(_, idx) => Some(usize::from(*idx)),
            })
        }
        match self.nodes[node_ref.0] {
            Node::Token(_, idx) => self.spans[usize::from(idx)].clone(),
            Node::Rule(_, end_offset) => {
                let end = node_ref.0 + usize::from(end_offset);
                let first = find_token(self.nodes[node_ref.0 + 1..=end].iter());
                let last = find_token(self.nodes[node_ref.0 + 1..=end].iter().rev());
                if let (Some(first), Some(last)) = (first, last) {
                    self.spans[first].start..self.spans[last].end
                } else {
                    let offset = find_token(self.nodes[..node_ref.0].iter().rev())
                        .map_or(0, |before| self.spans[before].end);
                    offset..offset
                }
            }
        }
    }
    pub fn match_token(&self, node_ref: NodeRef, matched_token: Token) -> Option<Span> {
        match self.nodes[node_ref.0] {
            Node::Token(token, idx) if token == matched_token => {
                Some(self.spans[usize::from(idx)].clone())
            }
            _ => None,
        }
    }
    pub fn match_rule(&self, node_ref: NodeRef, matched_rule: Rule) -> bool {
        matches!(self.nodes[node_ref.0], Node::Rule(rule, _) if rule == matched_rule)
    }
}
