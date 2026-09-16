//! Two phases: source-backed syntax blocks, then resolved data tables.
use crate::{
    DataLimits,
    source::{Diagnostic, SourceId},
};
use alloc::vec::Vec;
mod admission;
mod assemble;
mod build;
mod input;
mod lexer;
mod parse;
mod plan;
mod scalar;
mod structure;
mod validate;
pub use crate::json::text::ParseCtx;
pub use plan::{TomlKind, TomlNode, TomlPlan};

#[derive(Debug)]
pub struct TomlStructure<'a> {
    src: &'a str,
    source: SourceId,
    limits: DataLimits,
    raw: structure::Plan,
}
/// Syntax only. Header and dotted-key names remain unresolved source spans.
pub fn parse_structure(
    source: SourceId,
    input: &str,
    limits: DataLimits,
) -> Result<TomlStructure<'_>, Vec<Diagnostic>> {
    let raw = parse::parse(source, input, limits).map_err(|error| vec![error])?;
    Ok(TomlStructure {
        src: input,
        source,
        limits,
        raw,
    })
}
impl<'a> TomlStructure<'a> {
    /// Decode, resolve table identities, enforce final graph limits and collect
    /// independent semantic diagnostics. Never publish a partially valid tree.
    pub fn validate(self) -> Result<(TomlPlan, ParseCtx<'a>), Vec<Diagnostic>> {
        validate::validate(self)
    }
}
#[cfg(test)]
mod tests;
