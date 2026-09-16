use super::*;
use crate::{
    json::{JsonKind, text::TextSpan},
    source::Location,
};
use alloc::borrow::Cow;

#[test]
fn registered_json_moves_input_and_keeps_text_as_spans() {
    let text = String::from(r#"{"plain":"original","\u0061":"\uD83D\uDE00"}"#);
    let original = text.as_ptr();
    let mut sources = SourceDatabase::default();
    let id = sources.try_add_data("input.json", text).unwrap();
    let file = sources.get(id);
    assert!(file.text().document().is_none());
    assert_eq!(file.text().contiguous().unwrap().as_ptr(), original);
    assert!(matches!(
        file.slice(Location::from_usize(id, 0..1).unwrap()),
        Some(Cow::Borrowed("{"))
    ));
    let ParsedData::Json { plan, decoded } = parse_registered(&sources, id, Format::Json).unwrap()
    else {
        panic!("span plan")
    };
    assert_eq!(
        decoded.len(),
        5,
        "only the escaped key and emoji are decoded"
    );
    let JsonKind::Object(fields) = &plan.nodes[plan.root.index()].kind else {
        panic!("object")
    };
    let source = file.text().contiguous().unwrap();
    assert_eq!(fields[0].0.resolve(source, &decoded), "a");
    assert_eq!(fields[1].0.resolve(source, &decoded), "plain");
    let JsonKind::String(span) = &plan.nodes[fields[1].1.value.index()].kind else {
        panic!("string")
    };
    let TextSpan::Source(range) = span else {
        panic!("ordinary string must borrow original input")
    };
    assert_eq!(
        span.resolve(source, &decoded).as_ptr(),
        source[range.clone()].as_ptr()
    );
    assert_eq!(span.resolve(source, &decoded), "original");
    sources.try_add_data("later.json", "null".into()).unwrap();
    assert_eq!(
        sources.get(id).text().contiguous().unwrap().as_ptr(),
        original
    );
}

#[test]
fn contiguous_source_preserves_coordinates_and_reusable_slot() {
    let mut sources = SourceDatabase::default();
    let id = sources
        .try_add_data("mixed.json", "[\r\n\"é\",\r42\n]".into())
        .unwrap();
    let file = sources.get(id);
    assert_eq!(file.offset(2, 3), Some(6));
    assert_eq!(file.offset(3, 1), Some(9));
    assert_eq!(file.position(9).line, 3);
    let loc = Location::from_usize(id, 3..7).unwrap();
    assert_eq!(file.byte_location(file.coordinates(loc)), Some(loc));
    let text = String::from("[true]");
    let pointer = text.as_ptr();
    sources
        .replace_unreferenced_data(id, "next.json", text)
        .unwrap();
    assert_eq!(
        sources.get(id).text().contiguous().unwrap().as_ptr(),
        pointer
    );
    assert!(parse_registered(&sources, id, Format::Json).is_ok());
}
