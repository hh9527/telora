use super::structure::Plan;
use crate::{
    DataLimits,
    source::{Diagnostic, Location, SourceId},
};
use alloc::string::String;
use core::ops::Range;

pub(super) struct Build {
    pub source: SourceId,
    pub plan: Plan,
    pub limits: DataLimits,
    nodes: usize,
    payload: usize,
}

impl Build {
    pub fn new(source: SourceId, limits: DataLimits) -> Self {
        Self {
            source,
            limits,
            plan: Plan::default(),
            nodes: 0,
            payload: 0,
        }
    }
    pub fn loc(&self, range: Range<usize>) -> Location {
        Location::from_usize(self.source, range).expect("registered YAML span")
    }
    pub fn error(&self, range: Range<usize>, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(message, self.loc(range))
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
    pub fn reserve(&mut self, depth: usize, loc: Location) -> Result<(), Diagnostic> {
        self.check(loc, "depth", depth, self.limits.depth)?;
        self.check(loc, "nodes", self.nodes + 1, self.limits.nodes)?;
        self.nodes += 1;
        Ok(())
    }
    pub fn slot(&self, count: usize, loc: Location) -> Result<(), Diagnostic> {
        self.check(loc, "container_size", count + 1, self.limits.container_size)
    }
    fn payload(&mut self, bytes: usize, loc: Location) -> Result<(), Diagnostic> {
        let next = self
            .payload
            .checked_add(bytes)
            .ok_or_else(|| Diagnostic::error("data payload accounting overflow", loc))?;
        self.check(loc, "payloads_bytes", next, self.limits.payloads_bytes)?;
        self.payload = next;
        Ok(())
    }
    pub fn admit(
        &mut self,
        length: &mut usize,
        bytes: usize,
        binary: bool,
        loc: Location,
    ) -> Result<(), Diagnostic> {
        let next = length
            .checked_add(bytes)
            .ok_or_else(|| Diagnostic::error("data payload length overflow", loc))?;
        let (name, limit) = if binary {
            ("bytes_len", self.limits.bytes_len)
        } else {
            ("string_len", self.limits.string_len)
        };
        self.check(loc, name, next, limit)?;
        self.payload(bytes, loc)?;
        *length = next;
        Ok(())
    }
    pub fn unsupported(&self, text: &str, loc: Location) -> Result<(), Diagnostic> {
        let message = match text.as_bytes().first() {
            Some(b'&') => "YAML anchors are not supported",
            Some(b'*') => "YAML aliases are not supported",
            _ => return Ok(()),
        };
        Err(Diagnostic::error(message, loc))
    }
}
