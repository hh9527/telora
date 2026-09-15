use alloc::{format, vec::Vec};
use codespan_reporting::diagnostic::{Diagnostic, Label};
use core::ops::Range;

pub(crate) const MAX_NESTING: usize = 32;

pub(super) enum Delimiter {
    Open(char),
    Close(char),
}

pub(super) fn check(
    boundaries: impl Iterator<Item = (Delimiter, Range<usize>)>,
) -> Option<Diagnostic<()>> {
    let mut expected = Vec::new();
    for (boundary, span) in boundaries {
        match boundary {
            Delimiter::Open(closer) => {
                if expected.len() == MAX_NESTING {
                    return Some(
                        Diagnostic::error()
                            .with_message(format!(
                                "data syntax nesting exceeds parser limit ({MAX_NESTING})"
                            ))
                            .with_label(Label::primary((), span)),
                    );
                }
                expected.push(closer);
            }
            Delimiter::Close(closer) if expected.last() == Some(&closer) => {
                expected.pop();
            }
            Delimiter::Close(_) => {}
        }
    }
    None
}
