//! Parse-0 limits syntax and literal payloads, without guessing table identity.
use crate::{
    DataLimits,
    source::{Diagnostic, Location},
};

pub(super) struct Admission {
    pub limits: DataLimits,
    payload: usize,
}
impl Admission {
    pub fn new(limits: DataLimits) -> Self {
        Self { limits, payload: 0 }
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
    pub fn string_size(&self, size: usize, loc: Location, value: bool) -> Result<(), Diagnostic> {
        self.check(loc, "string_len", size, self.limits.string_len)?;
        if value {
            self.check(
                loc,
                "payloads_bytes",
                self.payload.saturating_add(size),
                self.limits.payloads_bytes,
            )?;
        }
        Ok(())
    }
    pub fn payload(&mut self, size: usize, loc: Location) -> Result<(), Diagnostic> {
        let next = self.payload.saturating_add(size);
        self.check(loc, "payloads_bytes", next, self.limits.payloads_bytes)?;
        self.payload = next;
        Ok(())
    }
}
