//! Parse-1: decode once into shared buffers and accumulate semantic errors.
use super::{
    scalar,
    structure::{self, Kind, Piece, Scalar, Text},
};
use crate::{
    json::{DataField, DataNodeId, text::TextSpan},
    source::{Diagnostic, Location},
};
use alloc::{string::String, vec::Vec};
use core::ops::Range;

#[derive(Clone, Debug)]
pub enum YamlKind {
    Int(i64),
    Float(f64),
    Null,
    Bool(bool),
    String(TextSpan),
    Bytes(Range<usize>),
    Array(Vec<DataNodeId>),
    Object(Vec<(TextSpan, DataField)>),
}
#[derive(Clone, Debug)]
pub struct YamlNode {
    pub kind: YamlKind,
    pub location: Location,
}
#[derive(Clone, Debug)]
pub struct YamlPlan {
    pub nodes: Vec<YamlNode>,
    pub root: DataNodeId,
}

#[derive(Debug)]
pub struct ParseCtx<'a> {
    src: &'a str,
    decoded: String,
    bytes: Vec<u8>,
}
impl<'a> ParseCtx<'a> {
    pub fn text(&self, span: &TextSpan) -> &str {
        span.resolve(self.src, &self.decoded)
    }
    pub fn bytes(&self, span: &Range<usize>) -> &[u8] {
        &self.bytes[span.clone()]
    }
    pub fn decoded_bytes(&self) -> usize {
        self.decoded.len() + self.bytes.len()
    }
    pub fn into_decoded(self) -> (String, Vec<u8>) {
        (self.decoded, self.bytes)
    }
    fn decode(&mut self, text: Text, loc: Location, pieces: &[Piece]) -> TextSpan {
        if let Text::Source(range) = text {
            return TextSpan::Source(range);
        }
        let start = self.decoded.len();
        match text {
            Text::Quoted(range) => {
                scalar::quoted(&self.src[range], loc, |piece, _| {
                    self.decoded.push_str(piece);
                    Ok(())
                })
                .expect("parse-0 validated escapes");
            }
            Text::Block(range) => {
                for piece in &pieces[range] {
                    match piece {
                        Piece::Source(range) => self.decoded.push_str(&self.src[range.clone()]),
                        Piece::Spaces(n) => self.decoded.extend(core::iter::repeat_n(' ', *n)),
                        Piece::Newlines(n) => self.decoded.extend(core::iter::repeat_n('\n', *n)),
                    }
                }
            }
            Text::Source(_) => unreachable!(),
        }
        TextSpan::Decoded(start..self.decoded.len())
    }
}

pub(super) fn validate(
    raw: structure::Plan,
    src: &str,
) -> Result<(YamlPlan, ParseCtx<'_>), Vec<Diagnostic>> {
    let mut ctx = ParseCtx {
        src,
        decoded: String::new(),
        bytes: Vec::new(),
    };
    let mut diagnostics = raw.diagnostics;
    // Equal-sized, equally aligned slots let Vec reuse its allocation during
    // conversion. Child and field vectors are moved, not copied.
    let nodes = raw
        .nodes
        .into_iter()
        .map(|node| {
            let kind = match node.kind {
                Kind::Scalar(Scalar::Null) => YamlKind::Null,
                Kind::Scalar(Scalar::Bool(b)) => YamlKind::Bool(b),
                Kind::Scalar(Scalar::String(text)) => {
                    YamlKind::String(ctx.decode(text, node.location, &raw.pieces))
                }
                Kind::Scalar(Scalar::Bytes(range)) => {
                    let start = ctx.bytes.len();
                    scalar::binary(&src[range], node.location, |bytes| {
                        ctx.bytes.extend_from_slice(bytes);
                        Ok(())
                    })
                    .expect("parse-0 validated base64");
                    YamlKind::Bytes(start..ctx.bytes.len())
                }
                Kind::Scalar(Scalar::Number { range, float }) => {
                    let text = &src[range];
                    let result = if float {
                        match scalar::normalize_number(text).parse::<f64>() {
                            Ok(n) if n.is_finite() => Ok(YamlKind::Float(n)),
                            Ok(_) => Err("YAML Float must be finite"),
                            Err(_)
                                if matches!(
                                    text,
                                    ".inf"
                                        | ".Inf"
                                        | ".INF"
                                        | "-.inf"
                                        | "-.Inf"
                                        | "-.INF"
                                        | ".nan"
                                        | ".NaN"
                                        | ".NAN"
                                ) =>
                            {
                                Err("YAML Float must be finite")
                            }
                            Err(_) => Err("invalid YAML Float"),
                        }
                    } else {
                        scalar::parse_yaml_int(text).map(YamlKind::Int)
                    };
                    match result {
                        Ok(kind) => kind,
                        Err(message) => {
                            diagnostics.push(Diagnostic::error(message, node.location));
                            YamlKind::Null
                        }
                    }
                }
                Kind::Array(items) => YamlKind::Array(items),
                Kind::Object(fields) => {
                    let mut fields: Vec<_> = fields
                        .into_iter()
                        .map(|(text, field)| {
                            (ctx.decode(text, field.key_location, &raw.pieces), field)
                        })
                        .collect();
                    fields.sort_by(|(a, _), (b, _)| ctx.text(a).cmp(ctx.text(b)));
                    let mut first = 0;
                    for index in 1..fields.len() {
                        if ctx.text(&fields[first].0) == ctx.text(&fields[index].0) {
                            diagnostics.push(
                                Diagnostic::error(
                                    format!("duplicate YAML key {:?}", ctx.text(&fields[index].0)),
                                    fields[index].1.key_location,
                                )
                                .with_secondary("first defined here", fields[first].1.key_location),
                            );
                        } else {
                            first = index;
                        }
                    }
                    YamlKind::Object(fields)
                }
            };
            YamlNode {
                kind,
                location: node.location,
            }
        })
        .collect();
    if !diagnostics.is_empty() {
        diagnostics.sort_by_key(|d| d.labels[0].location.start);
        return Err(diagnostics);
    }
    Ok((
        YamlPlan {
            nodes,
            root: raw.root.expect("parsed root"),
        },
        ctx,
    ))
}

#[cfg(test)]
impl YamlPlan {
    pub(crate) fn into_owned(
        self,
        source: &str,
        decoded: &str,
        bytes: &[u8],
    ) -> crate::json::ValidatedDataPlan {
        use crate::json::{DataScalar as S, ValidatedDataPlan};
        let mut plan = ValidatedDataPlan::default();
        for node in self.nodes {
            let scalar = match node.kind {
                YamlKind::Int(n) => S::Int(n),
                YamlKind::Float(n) => S::Float(n),
                YamlKind::Null => S::Null,
                YamlKind::Bool(b) => S::Bool(b),
                YamlKind::String(span) => S::String(span.resolve(source, decoded).into()),
                YamlKind::Bytes(range) => S::Bytes(bytes[range].into()),
                YamlKind::Array(items) => {
                    plan.array(items, node.location);
                    continue;
                }
                YamlKind::Object(fields) => {
                    plan.object(
                        fields
                            .into_iter()
                            .map(|(k, v)| (k.resolve(source, decoded).into(), v))
                            .collect(),
                        node.location,
                    );
                    continue;
                }
            };
            plan.scalar(scalar, node.location);
        }
        plan.set_root(self.root);
        plan.postordered = true;
        plan
    }
}
