use super::*;
use crate::ast::located;
use crate::source::{Location, SourceDatabase, TextRange};
use crate::value::Atom;
use std::collections::BTreeMap;

fn location(start: u32) -> Location {
    let mut sources = SourceDatabase::default();
    let source = sources.add("test.telora", "");
    Location::new(source, TextRange::new(start, start + 1).unwrap())
}

fn pattern(value: PatternKind, start: u32) -> Pattern {
    located(value, location(start))
}

fn binding(name: &str, start: u32) -> Pattern {
    pattern(
        PatternKind::Binding(located(name.to_owned(), location(start))),
        start,
    )
}

include!("part-01.rs");
