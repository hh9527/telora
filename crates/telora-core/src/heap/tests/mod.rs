use super::*;

fn location(name: &str, range: std::ops::Range<usize>) -> Loc {
    let mut sources = crate::SourceDatabase::default();
    let source = sources.add(name, "0123456789");
    Loc::from_usize(source, range).unwrap()
}

fn rv(value: DecodedValue) -> Val {
    value.into()
}

include!("part-01.rs");
