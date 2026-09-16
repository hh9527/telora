use crate::{
    DataLimits,
    json::{DataField, DataNodeId, DataPlanNodeKind, ValidatedDataPlan},
    source::{Diagnostic, Location},
};
use alloc::{collections::BTreeMap, string::String, vec::Vec};

#[derive(Default)]
pub(super) struct Meta {
    pub depth: usize,
    pub explicit: bool,
    pub sealed: bool,
    pub table_array: bool,
}

pub(super) struct Build {
    pub plan: ValidatedDataPlan,
    pub meta: Vec<Meta>,
    pub limits: DataLimits,
    pub payload: usize,
}

impl Build {
    pub fn new(limits: DataLimits) -> Self {
        Self {
            plan: ValidatedDataPlan::default(),
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
        let id = match kind {
            DataPlanNodeKind::Scalar(value) => self.plan.scalar(value, loc),
            DataPlanNodeKind::Array(values) => self.plan.array(values, loc),
            DataPlanNodeKind::Object(fields) => self.plan.object(fields, loc),
        };
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
    pub fn fields(&self, id: DataNodeId) -> &BTreeMap<String, DataField> {
        let DataPlanNodeKind::Object(fields) = &self.plan.node(id).kind else {
            unreachable!("table id")
        };
        fields
    }
    pub fn fields_mut(&mut self, id: DataNodeId) -> &mut BTreeMap<String, DataField> {
        let DataPlanNodeKind::Object(fields) = &mut self.plan.node_mut(id).kind else {
            unreachable!("table id")
        };
        fields
    }
    pub fn conflict(
        &self,
        message: impl Into<String>,
        loc: Location,
        previous: Location,
    ) -> Diagnostic {
        Diagnostic::error(message, loc).with_secondary("first defined here", previous)
    }
    pub fn mutable(&self, id: DataNodeId, loc: Location) -> Result<(), Diagnostic> {
        if self.meta[id.index()].sealed {
            Err(self.conflict(
                "cannot extend an inline TOML table",
                loc,
                self.plan.node(id).location,
            ))
        } else {
            Ok(())
        }
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
    pub fn string_size(&self, size: usize, loc: Location, value: bool) -> Result<(), Diagnostic> {
        self.check(loc, "string_len", size, self.limits.string_len)?;
        if value {
            let total = self
                .payload
                .checked_add(size)
                .ok_or_else(|| Diagnostic::error("data payload accounting overflow", loc))?;
            self.check(loc, "payloads_bytes", total, self.limits.payloads_bytes)?;
        }
        Ok(())
    }
    pub fn field_slot(
        &mut self,
        id: DataNodeId,
        key: &str,
        loc: Location,
    ) -> Result<(), Diagnostic> {
        self.mutable(id, loc)?;
        if let Some(field) = self.fields(id).get(key) {
            return Err(self.conflict(
                format!("duplicate TOML key {key:?}"),
                loc,
                field.key_location,
            ));
        }
        self.check(
            loc,
            "container_size",
            self.fields(id).len() + 1,
            self.limits.container_size,
        )?;
        self.payload(key.len(), loc)
    }
    pub fn insert(&mut self, id: DataNodeId, key: String, loc: Location, value: DataNodeId) {
        self.fields_mut(id).insert(
            key,
            DataField {
                value,
                key_location: loc,
            },
        );
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
        path: Vec<(String, Location)>,
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
                        return Err(self.conflict(
                            format!("TOML key {key:?} is not a table"),
                            loc,
                            field.key_location,
                        ));
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
        mut path: Vec<(String, Location)>,
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
            return Err(self.conflict(
                format!("TOML table {key:?} is already defined or has a conflicting type"),
                loc,
                field.key_location,
            ));
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
