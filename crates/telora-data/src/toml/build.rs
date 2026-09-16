use super::{
    ParseCtx,
    plan::{Kind as DataPlanNodeKind, Plan, Scalar},
};
use crate::json::text::TextSpan;
use crate::{
    DataLimits,
    json::{DataField, DataNodeId},
    source::{Diagnostic, Location},
};
use alloc::{collections::BTreeMap, vec::Vec};

#[derive(Default)]
pub(super) struct Meta {
    pub depth: usize,
    pub explicit: bool,
    pub sealed: bool,
    pub table_array: bool,
    pub slots: usize,
}

pub(super) struct Build<'a> {
    keys: &'a [TextSpan],
    ctx: &'a ParseCtx<'a>,
    pub diagnostics: Vec<Diagnostic>,
    pub plan: Plan,
    pub meta: Vec<Meta>,
    pub limits: DataLimits,
    pub payload: usize,
}

impl<'a> Build<'a> {
    pub fn new(keys: &'a [TextSpan], ctx: &'a ParseCtx<'a>, limits: DataLimits) -> Self {
        Self {
            keys,
            ctx,
            diagnostics: Vec::new(),
            plan: Plan::default(),
            meta: Vec::new(),
            limits,
            payload: 0,
        }
    }
    pub fn check(
        &self,
        loc: Location,
        name: &str,
        actual: usize,
        limit: usize,
    ) -> Result<(), Diagnostic> {
        if actual > limit {
            Err(Diagnostic::error(
                format!("data source exceeds {name} limit ({actual} > {limit})"),
                loc,
            ))
        } else {
            Ok(())
        }
    }
    pub fn depth(&self, id: DataNodeId) -> usize {
        self.meta[id.index()].depth
    }
    pub fn reserve(&self, depth: usize, loc: Location) -> Result<(), Diagnostic> {
        self.check(loc, "depth", depth, self.limits.depth)?;
        self.check(loc, "nodes", self.meta.len() + 1, self.limits.nodes)
    }
    pub fn node(
        &mut self,
        kind: DataPlanNodeKind,
        depth: usize,
        loc: Location,
    ) -> Result<DataNodeId, Diagnostic> {
        self.reserve(depth, loc)?;
        if let DataPlanNodeKind::Scalar(
            Scalar::String(text) | Scalar::Temporal { value: text, .. },
        ) = &kind
        {
            self.payload(self.ctx.text(text).len(), loc)?;
        }
        let id = self.plan.push(kind, loc);
        self.meta.push(Meta {
            depth,
            ..Meta::default()
        });
        Ok(id)
    }
    pub fn table(
        &mut self,
        depth: usize,
        loc: Location,
        explicit: bool,
    ) -> Result<DataNodeId, Diagnostic> {
        let id = self.node(DataPlanNodeKind::Object(BTreeMap::new()), depth, loc)?;
        self.meta[id.index()].explicit = explicit;
        Ok(id)
    }
    pub fn fields(&self, id: DataNodeId) -> &BTreeMap<usize, DataField> {
        let DataPlanNodeKind::Object(fields) = &self.plan.node(id).kind else {
            unreachable!("table id")
        };
        fields
    }
    pub fn fields_mut(&mut self, id: DataNodeId) -> &mut BTreeMap<usize, DataField> {
        let DataPlanNodeKind::Object(fields) = &mut self.plan.node_mut(id).kind else {
            unreachable!("table id")
        };
        fields
    }
    pub fn conflict(
        &mut self,
        message: &'static str,
        key: Option<usize>,
        loc: Location,
        previous: Location,
    ) {
        let message = match key {
            Some(key) => {
                let key = self.ctx.text(&self.keys[key]);
                match message {
                    "duplicate TOML key" => format!("duplicate TOML key {key:?}"),
                    "TOML key is not a table" => format!("TOML key {key:?} is not a table"),
                    _ => format!("TOML table {key:?} is already defined or has a conflicting type"),
                }
            }
            None => message.into(),
        };
        self.diagnostics
            .push(Diagnostic::error(message, loc).with_secondary("first defined here", previous));
    }
    pub fn mutable(&mut self, id: DataNodeId, loc: Location) -> Result<(), Diagnostic> {
        if self.meta[id.index()].sealed {
            self.conflict(
                "cannot extend an inline TOML table",
                None,
                loc,
                self.plan.node(id).location,
            );
        }
        Ok(())
    }
    pub fn payload(&mut self, size: usize, loc: Location) -> Result<(), Diagnostic> {
        let total = self
            .payload
            .checked_add(size)
            .ok_or_else(|| Diagnostic::error("data payload accounting overflow", loc))?;
        self.check(loc, "payloads_bytes", total, self.limits.payloads_bytes)?;
        self.payload = total;
        Ok(())
    }
    pub fn field_slot(
        &mut self,
        id: DataNodeId,
        key: &usize,
        loc: Location,
    ) -> Result<(), Diagnostic> {
        self.mutable(id, loc)?;
        if let Some(field) = self.fields(id).get(key) {
            let previous = field.key_location;
            self.conflict("duplicate TOML key", Some(*key), loc, previous);
        }
        self.check(
            loc,
            "container_size",
            self.meta[id.index()].slots + 1,
            self.limits.container_size,
        )?;
        self.meta[id.index()].slots += 1;
        self.payload(self.ctx.text(&self.keys[*key]).len(), loc)
    }
    pub fn insert(&mut self, id: DataNodeId, key: usize, loc: Location, value: DataNodeId) {
        self.fields_mut(id).entry(key).or_insert(DataField {
            value,
            key_location: loc,
        });
    }
    pub fn array_slot(&self, id: DataNodeId, loc: Location) -> Result<(), Diagnostic> {
        let DataPlanNodeKind::Array(items) = &self.plan.node(id).kind else {
            unreachable!("array id")
        };
        self.check(
            loc,
            "container_size",
            items.len() + 1,
            self.limits.container_size,
        )
    }
    pub fn push(&mut self, id: DataNodeId, value: DataNodeId) {
        let DataPlanNodeKind::Array(items) = &mut self.plan.node_mut(id).kind else {
            unreachable!("array id")
        };
        items.push(value);
    }
    /// Resolve a path once; newly constructed tables have their final tree depth.
    pub fn path(
        &mut self,
        mut id: DataNodeId,
        path: Vec<(usize, Location)>,
        dotted: bool,
    ) -> Result<DataNodeId, Diagnostic> {
        for (key, loc) in path {
            self.mutable(id, loc)?;
            if let Some(field) = self.fields(id).get(&key) {
                let child = field.value;
                id = match &self.plan.node(child).kind {
                    DataPlanNodeKind::Object(_) => child,
                    DataPlanNodeKind::Array(items) if self.meta[child.index()].table_array => {
                        *items.last().expect("nonempty table array")
                    }
                    _ => {
                        let previous = field.key_location;
                        self.conflict("TOML key is not a table", Some(key), loc, previous);
                        self.table(self.depth(id) + 1, loc, dotted)?
                    }
                };
            } else {
                self.field_slot(id, &key, loc)?;
                let child = self.table(self.depth(id) + 1, loc, dotted)?;
                self.insert(id, key, loc, child);
                id = child;
            }
        }
        Ok(id)
    }
    pub fn header(
        &mut self,
        root: DataNodeId,
        mut path: Vec<(usize, Location)>,
        array: bool,
        loc: Location,
    ) -> Result<DataNodeId, Diagnostic> {
        let (key, key_loc) = path.pop().expect("nonempty path");
        let parent = self.path(root, path, false)?;
        self.mutable(parent, loc)?;
        if let Some(field) = self.fields(parent).get(&key) {
            let id = field.value;
            if array && self.meta[id.index()].table_array {
                self.array_slot(id, loc)?;
                let table = self.table(self.depth(id) + 1, loc, true)?;
                self.push(id, table);
                return Ok(table);
            }
            if !array
                && matches!(self.plan.node(id).kind, DataPlanNodeKind::Object(_))
                && !self.meta[id.index()].explicit
                && !self.meta[id.index()].sealed
            {
                self.meta[id.index()].explicit = true;
                return Ok(id);
            }
            let previous = field.key_location;
            self.conflict(
                "TOML table is already defined or has a conflicting type",
                Some(key),
                loc,
                previous,
            );
            return self.table(self.depth(parent) + 1 + usize::from(array), loc, true);
        }
        self.field_slot(parent, &key, key_loc)?;
        if array {
            let id = self.node(
                DataPlanNodeKind::Array(Vec::new()),
                self.depth(parent) + 1,
                loc,
            )?;
            self.meta[id.index()].table_array = true;
            self.array_slot(id, loc)?;
            let table = self.table(self.depth(id) + 1, loc, true)?;
            self.push(id, table);
            self.insert(parent, key, key_loc, id);
            Ok(table)
        } else {
            let table = self.table(self.depth(parent) + 1, loc, true)?;
            self.insert(parent, key, key_loc, table);
            Ok(table)
        }
    }
}
