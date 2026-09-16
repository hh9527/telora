use crate::{
    DataLimits,
    json::ValidatedDataPlan,
    source::{Diagnostic, Location, SourceId},
};
use alloc::{string::String, vec::Vec};
use core::ops::Range;

pub(super) struct Build {
    pub source: SourceId,
    pub plan: ValidatedDataPlan,
    pub limits: DataLimits,
    nodes: usize,
    payload: usize,
}

impl Build {
    pub fn new(source: SourceId, limits: DataLimits) -> Self {
        Self {
            source,
            limits,
            plan: ValidatedDataPlan::default(),
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
    pub fn append(
        &mut self,
        output: &mut String,
        text: &str,
        loc: Location,
    ) -> Result<(), Diagnostic> {
        let next = output
            .len()
            .checked_add(text.len())
            .ok_or_else(|| Diagnostic::error("data string length overflow", loc))?;
        self.check(loc, "string_len", next, self.limits.string_len)?;
        self.payload(text.len(), loc)?;
        output.push_str(text);
        Ok(())
    }
    pub fn bytes(
        &mut self,
        output: &mut Vec<u8>,
        bytes: &[u8],
        loc: Location,
    ) -> Result<(), Diagnostic> {
        let next = output
            .len()
            .checked_add(bytes.len())
            .ok_or_else(|| Diagnostic::error("data bytes length overflow", loc))?;
        self.check(loc, "bytes_len", next, self.limits.bytes_len)?;
        self.payload(bytes.len(), loc)?;
        output.extend_from_slice(bytes);
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
