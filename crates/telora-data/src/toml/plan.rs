//! Resolved data graph. Text refers to source or the one decoded buffer.
use crate::{
    json::{DataField, DataNodeId, TemporalKind, text::TextSpan},
    source::Location,
};
use alloc::{collections::BTreeMap, vec::Vec};

#[derive(Clone, Debug)]
pub enum TomlKind {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(TextSpan),
    Temporal { kind: TemporalKind, value: TextSpan },
    Array(Vec<DataNodeId>),
    Object(Vec<(TextSpan, DataField)>),
}
#[derive(Clone, Debug)]
pub struct TomlNode {
    pub kind: TomlKind,
    pub location: Location,
}
#[derive(Clone, Debug)]
pub struct TomlPlan {
    pub nodes: Vec<TomlNode>,
    pub root: DataNodeId,
}

#[derive(Debug)]
pub(super) enum Scalar {
    Int(i64),
    Float(f64),
    Bool(bool),
    String(TextSpan),
    Temporal { kind: TemporalKind, value: TextSpan },
}
#[derive(Debug)]
pub(super) enum Kind {
    Scalar(Scalar),
    Array(Vec<DataNodeId>),
    Object(BTreeMap<usize, DataField>),
}
#[derive(Debug)]
pub(super) struct Node {
    pub kind: Kind,
    pub location: Location,
}
#[derive(Debug, Default)]
pub(super) struct Plan {
    pub nodes: Vec<Node>,
}
impl Plan {
    pub fn push(&mut self, kind: Kind, location: Location) -> DataNodeId {
        let id = DataNodeId(self.nodes.len());
        self.nodes.push(Node { kind, location });
        id
    }
    pub fn node(&self, id: DataNodeId) -> &Node {
        &self.nodes[id.index()]
    }
    pub fn node_mut(&mut self, id: DataNodeId) -> &mut Node {
        &mut self.nodes[id.index()]
    }
    pub fn finish(
        self,
        root: DataNodeId,
        keys: &[TextSpan],
        ctx: &super::ParseCtx<'_>,
    ) -> TomlPlan {
        let nodes = self
            .nodes
            .into_iter()
            .map(|node| TomlNode {
                location: node.location,
                kind: match node.kind {
                    Kind::Scalar(Scalar::Int(n)) => TomlKind::Int(n),
                    Kind::Scalar(Scalar::Float(n)) => TomlKind::Float(n),
                    Kind::Scalar(Scalar::Bool(b)) => TomlKind::Bool(b),
                    Kind::Scalar(Scalar::String(s)) => TomlKind::String(s),
                    Kind::Scalar(Scalar::Temporal { kind, value }) => {
                        TomlKind::Temporal { kind, value }
                    }
                    Kind::Array(items) => TomlKind::Array(items),
                    Kind::Object(fields) => {
                        let mut fields: Vec<_> = fields
                            .into_iter()
                            .map(|(key, field)| (keys[key].clone(), field))
                            .collect();
                        fields.sort_unstable_by(|(a, _), (b, _)| ctx.text(a).cmp(ctx.text(b)));
                        TomlKind::Object(fields)
                    }
                },
            })
            .collect();
        TomlPlan { nodes, root }.postorder()
    }
}
impl TomlPlan {
    /// Compute a permutation, relink edges, then swap slots in place. No
    /// recursive traversal or second owned node/payload graph is needed.
    fn postorder(mut self) -> Self {
        let mut stack = vec![(self.root, false)];
        let mut mapping = vec![0; self.nodes.len()];
        let mut next = 0;
        while let Some((id, done)) = stack.pop() {
            if done {
                mapping[id.index()] = next;
                next += 1;
                continue;
            }
            stack.push((id, true));
            match &self.nodes[id.index()].kind {
                TomlKind::Array(items) => stack.extend(items.iter().rev().map(|id| (*id, false))),
                TomlKind::Object(fields) => {
                    stack.extend(fields.iter().rev().map(|(_, f)| (f.value, false)))
                }
                _ => {}
            }
        }
        assert_eq!(next, self.nodes.len(), "valid TOML graph is a tree");
        self.root = DataNodeId(mapping[self.root.index()]);
        for node in &mut self.nodes {
            match &mut node.kind {
                TomlKind::Array(items) => {
                    for id in items {
                        *id = DataNodeId(mapping[id.index()]);
                    }
                }
                TomlKind::Object(fields) => {
                    for (_, field) in fields {
                        field.value = DataNodeId(mapping[field.value.index()]);
                    }
                }
                _ => {}
            }
        }
        for index in 0..self.nodes.len() {
            while mapping[index] != index {
                let target = mapping[index];
                self.nodes.swap(index, target);
                mapping.swap(index, target);
            }
        }
        self
    }
}
