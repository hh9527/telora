use super::*;
use crate::{SourceDatabase, TextRange};

fn location(offset: u32) -> Location {
    let mut sources = SourceDatabase::default();
    let source = sources.add("lineage.telora", "0123456789");
    Location::new(source, TextRange::new(offset, offset + 1).unwrap())
}

fn limits() -> FailureLimits {
    FailureLimits::new(8, 3, 8)
}

fn root(arena: &mut FailureArena<&'static str>, message: &'static str) -> FailureId {
    arena
        .root(FailureClass::Recoverable, message)
        .unwrap()
        .failure()
        .unwrap()
}

include!("part-01.rs");
