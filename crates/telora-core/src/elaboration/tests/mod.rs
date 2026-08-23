use super::*;
use crate::source::{SourceDatabase, TextRange};

fn location() -> Location {
    let mut sources = SourceDatabase::default();
    let source = sources.add("test", "input?");
    Location::new(source, TextRange::new(0, 6).unwrap())
}

include!("part-01.rs");
