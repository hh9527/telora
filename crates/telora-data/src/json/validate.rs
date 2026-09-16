//! Parse-1 resolves payloads into one append-only buffer and validates keys.
use super::{
    lexer::{Kind, Scanner},
    structure::{JsonKind, JsonNode, JsonPlan, RawKind, RawPlan, StringSpan},
    text::{ParseCtx, TextSpan},
};
use crate::source::Diagnostic;
use alloc::vec::Vec;

fn decode(ctx: &mut ParseCtx<'_>, value: StringSpan) -> TextSpan {
    if !value.escaped {
        return TextSpan::Source(value.range);
    }
    let start = ctx.decoded.len();
    // Parse-0 has already checked escapes, surrogate pairing, and quotas.
    let quoted = &ctx.src[value.range.start - 1..value.range.end + 1];
    let mut scanner = Scanner::new(core::iter::once(quoted));
    let mut high = None;
    loop {
        match scanner.next().expect("validated string").kind {
            Kind::QStart => {}
            Kind::QEnd => break,
            Kind::Text(text) => ctx.decoded.push_str(text),
            Kind::EscChar(ch) => ctx.decoded.push(ch),
            Kind::EscUtf16(first @ 0xd800..=0xdbff) => high = Some(first),
            Kind::EscUtf16(value) => {
                let code = match high.take() {
                    Some(first) => {
                        0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(value) - 0xdc00
                    }
                    None => u32::from(value),
                };
                ctx.decoded
                    .push(char::from_u32(code).expect("validated Unicode"));
            }
            _ => unreachable!("validated string"),
        }
    }
    TextSpan::Decoded(start..ctx.decoded.len())
}

pub(super) fn validate(raw: RawPlan, ctx: &mut ParseCtx<'_>) -> Result<JsonPlan, Vec<Diagnostic>> {
    let mut diagnostics = raw.diagnostics;
    // Equal-width raw/validated nodes allow Vec's in-place iterator collection
    // to reuse the arena. Child arrays and object field storage are moved too.
    let nodes = raw
        .nodes
        .into_iter()
        .map(|node| {
            let kind = match node.kind {
                RawKind::String(value) => JsonKind::String(decode(ctx, value)),
                RawKind::Number { range, float } => {
                    let text = &ctx.src[range];
                    let value = if float {
                        match text.parse::<f64>() {
                            Ok(n) if n.is_finite() => JsonKind::Float(n),
                            result => {
                                diagnostics.push(Diagnostic::error(
                                    if result.is_ok() {
                                        "JSON Float must be finite"
                                    } else {
                                        "invalid Float value"
                                    },
                                    node.location,
                                ));
                                JsonKind::Null
                            }
                        }
                    } else {
                        match text.parse() {
                            Ok(n) => JsonKind::Int(n),
                            Err(_) => {
                                diagnostics.push(Diagnostic::error(
                                    "JSON integer is outside the i64 range",
                                    node.location,
                                ));
                                JsonKind::Null
                            }
                        }
                    };
                    value
                }
                RawKind::Bool(value) => JsonKind::Bool(value),
                RawKind::Null => JsonKind::Null,
                RawKind::Array(items) => JsonKind::Array(items),
                RawKind::Object(fields) => {
                    let mut fields: Vec<_> = fields
                        .into_iter()
                        .map(|(key, field)| (decode(ctx, key), field))
                        .collect();
                    // Stable sorting preserves the first occurrence for diagnostics.
                    fields.sort_by(|(a, _), (b, _)| ctx.text(a).cmp(ctx.text(b)));
                    let mut first = 0;
                    for index in 1..fields.len() {
                        if ctx.text(&fields[first].0) == ctx.text(&fields[index].0) {
                            diagnostics.push(
                                Diagnostic::error(
                                    format!(
                                        "duplicate JSON object key {:?}",
                                        ctx.text(&fields[index].0)
                                    ),
                                    fields[index].1.key_location,
                                )
                                .with_secondary("first defined here", fields[first].1.key_location),
                            );
                        } else {
                            first = index;
                        }
                    }
                    JsonKind::Object(fields)
                }
            };
            JsonNode {
                kind,
                location: node.location,
            }
        })
        .collect();
    if diagnostics.is_empty() {
        Ok(JsonPlan {
            nodes,
            root: raw.root.expect("parsed root"),
        })
    } else {
        diagnostics.sort_by_key(|diagnostic| diagnostic.labels[0].location.start);
        Err(diagnostics)
    }
}
