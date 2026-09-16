use crate::{
    DataLimits,
    json::ValidatedDataPlan,
    source::{Diagnostic, SourceDatabase, SourceId},
};
use alloc::vec::Vec;
mod build;
mod input;
mod lexer;
mod parse;
mod scalar;
#[cfg(test)]
mod tests;

pub(crate) fn parse_with_limits(
    sources: &SourceDatabase,
    source: SourceId,
    limits: DataLimits,
) -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
    parse::parse(sources, source, limits).map_err(|error| vec![error])
}
