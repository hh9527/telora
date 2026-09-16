use super::{
    ParseCtx, TomlPlan, TomlStructure,
    input::Input,
    plan::Scalar,
    structure::{self, Text},
};
use crate::{
    json::text::TextSpan,
    source::{Diagnostic, Location, SourceId},
};
use alloc::{collections::BTreeMap, vec::Vec};

pub(super) enum Kind {
    Scalar(Scalar),
    Array(Vec<usize>),
    Inline(Vec<structure::Assignment>),
}
pub(super) struct Node {
    pub kind: Kind,
    pub location: Location,
}

fn decode(text: Text, source: SourceId, ctx: &mut ParseCtx<'_>) -> TextSpan {
    if !text.escaped {
        return TextSpan::Source(text.range);
    }
    let start = ctx.decoded.len();
    let mut input = Input::new(source, ctx.src);
    input.offset = text.range.start;
    input
        .string(true, |piece, _, _| {
            ctx.decoded.push_str(piece);
            Ok(())
        })
        .expect("parse-0 validated string");
    TextSpan::Decoded(start..ctx.decoded.len())
}

pub(super) fn validate(
    parsed: TomlStructure<'_>,
) -> Result<(TomlPlan, ParseCtx<'_>), Vec<Diagnostic>> {
    let TomlStructure {
        src,
        source,
        limits,
        raw,
    } = parsed;
    let mut ctx = ParseCtx::new(src);
    let mut diagnostics = raw.diagnostics;
    let keys: Vec<_> = raw
        .keys
        .into_iter()
        .map(|key| (decode(key.text, source, &mut ctx), key.location))
        .collect();
    let nodes = raw
        .nodes
        .into_iter()
        .map(|node| {
            let kind = match node.kind {
                structure::Kind::Scalar(structure::Scalar::Bool(b)) => {
                    Kind::Scalar(Scalar::Bool(b))
                }
                structure::Kind::Scalar(structure::Scalar::String(text)) => {
                    Kind::Scalar(Scalar::String(decode(text, source, &mut ctx)))
                }
                structure::Kind::Scalar(structure::Scalar::Atom(range)) => {
                    let text = &src[range.clone()];
                    let result = if let Some(temporal) = super::scalar::parse_temporal(text) {
                        temporal.map(|(kind, parts)| {
                            let value = if parts.iter().flat_map(|s| s.bytes()).eq(text.bytes()) {
                                TextSpan::Source(range)
                            } else {
                                let start = ctx.decoded.len();
                                for part in parts {
                                    ctx.decoded.push_str(part);
                                }
                                TextSpan::Decoded(start..ctx.decoded.len())
                            };
                            Scalar::Temporal { kind, value }
                        })
                    } else {
                        super::scalar::parse_number(text)
                    };
                    Kind::Scalar(match result {
                        Ok(value) => value,
                        Err(message) => {
                            diagnostics.push(Diagnostic::error(message, node.location));
                            Scalar::Bool(false)
                        }
                    })
                }
                structure::Kind::Array(items) => Kind::Array(items),
                structure::Kind::Inline(fields) => Kind::Inline(fields),
            };
            Some(Node {
                kind,
                location: node.location,
            })
        })
        .collect();
    // All decoding is complete. These borrowed map keys cannot be invalidated
    // by buffer growth; neither interning nor table construction owns strings.
    let mut interned = BTreeMap::new();
    let mut canonical = Vec::new();
    let paths: Vec<_> = keys
        .iter()
        .map(|(text, loc)| {
            let next = canonical.len();
            let id = *interned.entry(ctx.text(text)).or_insert_with(|| {
                canonical.push(text.clone());
                next
            });
            (id, *loc)
        })
        .collect();
    let mut build = super::build::Build::new(&canonical, &ctx, limits);
    let root_loc = Location::from_usize(source, 0..src.len()).expect("input span");
    let root = super::assemble::assemble(&mut build, nodes, &paths, raw.tables, root_loc)
        .map_err(|error| vec![error])?;
    diagnostics.append(&mut build.diagnostics);
    if !diagnostics.is_empty() {
        diagnostics.sort_by_key(|d| d.labels[0].location.start);
        return Err(diagnostics);
    }
    let plan = build.plan.finish(root, &canonical, &ctx);
    Ok((plan, ctx))
}
