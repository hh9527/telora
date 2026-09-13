//! Fixed capture contract: count followed by {name pointer, optional, parsable}.
use alloc::{
    collections::BTreeSet,
    format,
    string::{String, ToString},
    vec::Vec,
};
use regex_syntax::hir::{Hir, HirKind};

pub(crate) fn required(hir: &Hir) -> BTreeSet<String> {
    match hir.kind() {
        HirKind::Capture(capture) => {
            let mut names = required(&capture.sub);
            if let Some(name) = &capture.name {
                names.insert(name.to_string());
            }
            names
        }
        HirKind::Concat(items) => items.iter().flat_map(required).collect(),
        HirKind::Alternation(items) => {
            let mut items = items.iter();
            let Some(first) = items.next() else {
                return BTreeSet::new();
            };
            items.fold(required(first), |names, item| {
                names.intersection(&required(item)).cloned().collect()
            })
        }
        HirKind::Repetition(repetition) if repetition.min != 0 => required(&repetition.sub),
        _ => BTreeSet::new(),
    }
}

pub(crate) unsafe fn validate(
    captures: &BTreeSet<String>,
    required: &BTreeSet<String>,
    packet: u32,
) -> Result<(), String> {
    use crate::{text::text, values::word};
    unsafe {
        let count = word(packet, 0);
        let fields = (0..count)
            .map(|index| packet + 4 + index * 12)
            .collect::<Vec<_>>();
        let expected = fields
            .iter()
            .map(|&field| text(word(field, 0)).to_string())
            .collect::<BTreeSet<_>>();
        if expected != *captures {
            let missing = expected.difference(captures).collect::<Vec<_>>();
            let extra = captures.difference(&expected).collect::<Vec<_>>();
            return Err(format!(
                "regex captures must match struct fields; missing captures {missing:?}, extra captures {extra:?}"
            ));
        }
        for field in fields {
            let name = text(word(field, 0));
            let optional = word(field, 4) != 0;
            if word(field, 8) == 0 {
                return Err(format!("regex field {name:?} is not string-parsable"));
            }
            if optional == required.contains(name) {
                return Err(format!(
                    "regex capture {name:?} is {}, but its field is {}",
                    if optional { "required" } else { "optional" },
                    if optional { "optional" } else { "required" }
                ));
            }
        }
        Ok(())
    }
}
