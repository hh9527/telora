use crate::{CallContext, NativeError, NativeType};
use regex_syntax::hir::{Hir, HirKind};
use std::collections::BTreeSet;

#[derive(Clone)]
struct CompiledRegex {
    pattern: String,
    regex: regex::Regex,
    captures: BTreeSet<String>,
    required: BTreeSet<String>,
}

impl PartialEq for CompiledRegex {
    fn eq(&self, other: &Self) -> bool {
        self.pattern == other.pattern
    }
}

impl Eq for CompiledRegex {}

fn regex_type(context: &CallContext<'_, '_>) -> Result<NativeType, NativeError> {
    context
        .value(context.upvalue(0)?)?
        .as_native_type()
        .cloned()
        .ok_or_else(|| NativeError::new("Regex native type is not linked"))
}

fn compiled_argument(
    context: &CallContext<'_, '_>,
    index: usize,
    native_type: &NativeType,
) -> Result<CompiledRegex, NativeError> {
    context
        .value(context.argument(index)?)?
        .as_opaque::<CompiledRegex>(native_type)
        .cloned()
        .ok_or_else(|| NativeError::new("expected std/regex#Regex"))
}

fn required_captures(hir: &Hir) -> BTreeSet<String> {
    match hir.kind() {
        HirKind::Capture(capture) => {
            let mut required = required_captures(&capture.sub);
            if let Some(name) = &capture.name {
                required.insert(name.to_string());
            }
            required
        }
        HirKind::Concat(items) => items.iter().fold(BTreeSet::new(), |mut all, item| {
            all.extend(required_captures(item));
            all
        }),
        HirKind::Alternation(items) => {
            let mut items = items.iter();
            let Some(first) = items.next() else {
                return BTreeSet::new();
            };
            items.fold(required_captures(first), |required, item| {
                required
                    .intersection(&required_captures(item))
                    .cloned()
                    .collect()
            })
        }
        HirKind::Repetition(repetition) if repetition.min == 0 => BTreeSet::new(),
        HirKind::Repetition(repetition) => required_captures(&repetition.sub),
        _ => BTreeSet::new(),
    }
}

fn compile_pattern(pattern: String) -> Result<CompiledRegex, NativeError> {
    let hir = regex_syntax::Parser::new()
        .parse(&pattern)
        .map_err(|error| NativeError::new(format!("invalid regular expression: {error}")))?;
    let regex = regex::Regex::new(&pattern)
        .map_err(|error| NativeError::new(format!("invalid regular expression: {error}")))?;
    let mut captures = BTreeSet::new();
    for (index, name) in regex.capture_names().enumerate().skip(1) {
        let name = name
            .ok_or_else(|| NativeError::new(format!("capture group {index} must have a name")))?;
        if !captures.insert(name.to_owned()) {
            return Err(NativeError::new(format!("duplicate capture name {name:?}")));
        }
    }
    Ok(CompiledRegex {
        pattern,
        regex,
        captures,
        required: required_captures(&hir),
    })
}

pub(crate) fn native_compile(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = regex_type(context)?;
    let pattern = context
        .value(context.argument(0)?)?
        .as_str()
        .ok_or_else(|| NativeError::new("std/regex.compile expects String"))?
        .to_owned();
    let compiled = compile_pattern(pattern.as_str().to_owned())?;
    context.set_opaque(context.result(), native_type, compiled)
}

pub(crate) fn native_is_match(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = regex_type(context)?;
    let compiled = compiled_argument(context, 0, &native_type)?;
    let input = context
        .value(context.argument(1)?)?
        .as_str()
        .ok_or_else(|| NativeError::new("std/regex.is_match expects String"))?
        .to_owned();
    context.set_atom(
        context.result(),
        if compiled.regex.is_match(&input) {
            "True"
        } else {
            "False"
        },
    )
}

pub(crate) fn native_prepare(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = regex_type(context)?;
    let (types, graph) = context
        .solved_image()
        .ok_or_else(|| NativeError::new("std/regex.prepare requires a linked type image"))?;
    let compiled = context
        .value(context.argument(0)?)?
        .as_opaque::<CompiledRegex>(&native_type)
        .ok_or_else(|| NativeError::new("expected Regex"))?;
    let property = context
        .value(context.argument(1)?)?
        .represented_type_id()
        .ok_or_else(|| NativeError::new("expected solved ParseBy metadata"))?;
    let owner = context
        .value(context.argument(2)?)?
        .represented_type_id()
        .ok_or_else(|| NativeError::new("expected solved target metadata"))?;
    solved_fields(compiled, owner, property, types, graph)?;
    return context.copy(context.result(), context.argument(0)?);
}

/// Capture validation consumes solved member identities and static property
/// presence. It does not evaluate nested properties or reconstruct descriptors.
fn solved_fields(
    compiled: &CompiledRegex,
    owner: crate::mir::TypeId,
    property: crate::mir::TypeId,
    types: &crate::type_image::TypeImage,
    graph: &crate::execution_graph::ExecutionGraph,
) -> Result<Vec<(String, crate::mir::TypeId)>, NativeError> {
    use crate::mir::{PropertySite, TypeConstructor as T};
    let body = types.layout(owner).map_or(owner, |layout| layout.body);
    let shape = &types.types[body.index()];
    let T::Record(names) = &shape.constructor else {
        return Err(NativeError::new(
            "std/regex.parse_by requires a struct type",
        ));
    };
    let expected = names.iter().cloned().collect::<BTreeSet<_>>();
    if expected != compiled.captures {
        let missing = expected.difference(&compiled.captures).collect::<Vec<_>>();
        let extra = compiled.captures.difference(&expected).collect::<Vec<_>>();
        return Err(NativeError::new(format!(
            "regex captures must match struct fields; missing captures {missing:?}, extra captures {extra:?}"
        )));
    }
    for (name, &ty) in names.iter().zip(&shape.arguments) {
        let field = &types.types[ty.index()];
        let optional = field.constructor == T::Option;
        let inner = if optional { field.arguments[0] } else { ty };
        if !matches!(
            types.types[inner.index()].constructor,
            T::Int | T::Float | T::String
        ) && graph
            .property(crate::execution_graph::PropertyKey {
                owner: inner,
                site: PropertySite::Type,
                property,
            })
            .is_none()
        {
            return Err(NativeError::new(format!(
                "regex field {name:?} is not string-parsable"
            )));
        }
        if optional == compiled.required.contains(name) {
            return Err(NativeError::new(format!(
                "regex capture {name:?} is {}, but its field is {}",
                if optional { "required" } else { "optional" },
                if optional { "optional" } else { "required" }
            )));
        }
    }
    Ok(names
        .iter()
        .cloned()
        .zip(shape.arguments.iter().copied())
        .collect())
}

/// Return offsets into the caller's original String. No captured payload is
/// copied into a Rust value tree, and every capture already has its TypeId.
pub(crate) fn solved_captures(
    regex: crate::ValueRef<'_>,
    input: &str,
    owner: crate::mir::TypeId,
    property: crate::mir::TypeId,
    types: &crate::type_image::TypeImage,
    graph: &crate::execution_graph::ExecutionGraph,
) -> Result<Vec<(String, crate::mir::TypeId, Option<std::ops::Range<usize>>)>, NativeError> {
    let native = regex
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("ParseBy has no Regex value"))?;
    let compiled = regex
        .as_opaque::<CompiledRegex>(native)
        .ok_or_else(|| NativeError::new("ParseBy has an invalid Regex value"))?;
    let fields = solved_fields(compiled, owner, property, types, graph)?;
    let captures = compiled
        .regex
        .captures(input)
        .ok_or_else(|| NativeError::new("input does not match regular expression"))?;
    Ok(fields
        .into_iter()
        .map(|(name, ty)| {
            let range = captures.name(&name).map(|capture| capture.range());
            (name, ty, range)
        })
        .collect())
}
