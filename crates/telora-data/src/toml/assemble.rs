//! Iterative table construction in parse-1. Source syntax is consumed once.
use super::{
    build::Build,
    plan::Kind,
    structure::{Assignment, Table},
    validate::{Kind as Syntax, Node},
};
use crate::{
    json::DataNodeId,
    source::{Diagnostic, Location},
};
use alloc::{vec::IntoIter, vec::Vec};

fn assignment(
    build: &mut Build<'_>,
    table: DataNodeId,
    path: Vec<usize>,
    keys: &[(usize, Location)],
) -> Result<(DataNodeId, usize, Location), Diagnostic> {
    let mut path: Vec<_> = path.into_iter().map(|id| keys[id]).collect();
    let (key, loc) = path.pop().expect("nonempty key path");
    let table = build.path(table, path, true)?;
    build.field_slot(table, &key, loc)?;
    Ok((table, key, loc))
}

pub(super) fn assemble(
    build: &mut Build<'_>,
    mut nodes: Vec<Option<Node>>,
    keys: &[(usize, Location)],
    tables: Vec<Table>,
    loc: Location,
) -> Result<DataNodeId, Diagnostic> {
    let root = build.table(1, loc, true)?;
    for table in tables {
        let current = match table.header {
            Some(header) => build.header(
                root,
                header.path.into_iter().map(|id| keys[id]).collect(),
                header.array,
                header.location,
            )?,
            None => root,
        };
        for field in table.items {
            let (table, key, loc) = assignment(build, current, field.path, keys)?;
            let value = value(build, &mut nodes, keys, field.value, build.depth(table) + 1)?;
            build.insert(table, key, loc, value);
        }
    }
    Ok(root)
}

enum Task {
    Value(usize, usize),
    Array(DataNodeId, IntoIter<usize>),
    Inline(DataNodeId, IntoIter<Assignment>),
    Push(DataNodeId),
    Field(DataNodeId, usize, Location),
}
fn value(
    build: &mut Build<'_>,
    nodes: &mut [Option<Node>],
    keys: &[(usize, Location)],
    root: usize,
    depth: usize,
) -> Result<DataNodeId, Diagnostic> {
    let mut tasks = vec![Task::Value(root, depth)];
    let mut result = None;
    while let Some(task) = tasks.pop() {
        match task {
            Task::Value(id, depth) => {
                let node = nodes[id].take().expect("syntax value consumed once");
                match node.kind {
                    Syntax::Scalar(scalar) => {
                        result = Some(build.node(Kind::Scalar(scalar), depth, node.location)?)
                    }
                    Syntax::Array(items) => {
                        let id = build.node(Kind::Array(Vec::new()), depth, node.location)?;
                        tasks.push(Task::Array(id, items.into_iter()));
                    }
                    Syntax::Inline(fields) => {
                        let id = build.table(depth, node.location, true)?;
                        tasks.push(Task::Inline(id, fields.into_iter()));
                    }
                }
            }
            Task::Array(id, mut items) => match items.next() {
                Some(child) => {
                    build.array_slot(id, nodes[child].as_ref().unwrap().location)?;
                    tasks.push(Task::Array(id, items));
                    tasks.push(Task::Push(id));
                    tasks.push(Task::Value(child, build.depth(id) + 1));
                }
                None => result = Some(id),
            },
            Task::Inline(id, mut fields) => match fields.next() {
                Some(field) => {
                    let (table, key, loc) = assignment(build, id, field.path, keys)?;
                    tasks.push(Task::Inline(id, fields));
                    tasks.push(Task::Field(table, key, loc));
                    tasks.push(Task::Value(field.value, build.depth(table) + 1));
                }
                None => {
                    build.meta[id.index()].sealed = true;
                    result = Some(id);
                }
            },
            Task::Push(id) => build.push(id, result.take().unwrap()),
            Task::Field(id, key, loc) => build.insert(id, key, loc, result.take().unwrap()),
        }
    }
    Ok(result.expect("completed data value"))
}
