use crate::{CallContext, NativeError, NativeType};
use regex_syntax::hir::{Hir, HirKind};
use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Clone)]
enum ParsePlan {
    String,
    Int,
    Float,
    Regex {
        compiled: CompiledRegex,
        fields: BTreeMap<String, FieldPlan>,
    },
}

#[derive(Clone)]
struct FieldPlan {
    plan: ParsePlan,
    optional: bool,
}

pub(crate) enum ParsedValue {
    String(String),
    Int(i64),
    Float(f64),
    None,
    Some(Box<Self>),
    Struct(Vec<(String, Self)>),
}

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

fn resolve_metadata(mut metadata: crate::ValueRef<'_>) -> Result<crate::ValueRef<'_>, NativeError> {
    for _ in 0..128 {
        if let Some(body) = metadata.declared_type_body() {
            metadata = body;
            continue;
        }
        if metadata.is_hidden_type_slot() {
            metadata = metadata
                .resolve_hidden_type_slot()
                .map_err(NativeError::new)?;
            continue;
        }
        return Ok(metadata);
    }
    Err(NativeError::new(
        "std/regex metadata resolution exceeds the recursive type limit",
    ))
}

fn option_payload(metadata: crate::ValueRef<'_>) -> Option<crate::ValueRef<'_>> {
    let metadata = resolve_metadata(metadata).ok()?;
    if metadata.dict_get("kind")?.as_atom()? != "Enum" {
        return None;
    }
    let variants = metadata.dict_get("variants")?;
    if variants.dict_fields()?.as_slice() != ["None", "Some"] {
        return None;
    }
    let none = resolve_metadata(variants.dict_get("None")?).ok()?;
    if none.as_atom()? != "None" {
        return None;
    }
    let payload = resolve_metadata(variants.dict_get("Some")?).ok()?;
    (!payload.as_atom().is_some_and(|atom| atom == "None")).then_some(payload)
}

fn attached_regex(
    mut metadata: crate::ValueRef<'_>,
    property_type: crate::TypeId,
) -> Result<Option<CompiledRegex>, NativeError> {
    for _ in 0..128 {
        if metadata.is_hidden_type_slot() {
            metadata = metadata
                .resolve_hidden_type_slot()
                .map_err(NativeError::new)?;
            continue;
        }
        break;
    }
    let Some(property) = metadata.type_property(property_type) else {
        return Ok(None);
    };
    let payload = property
        .dict_get("regex")
        .ok_or_else(|| NativeError::new("regex ParseBy property has no regex"))?;
    let native_type = payload
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("regex ParseBy property has an invalid payload"))?;
    if native_type.qualified_name() != "std/regex#Regex" {
        return Err(NativeError::new(
            "regex ParseBy property has an invalid payload",
        ));
    }
    payload
        .as_opaque::<CompiledRegex>(native_type)
        .cloned()
        .map(Some)
        .ok_or_else(|| NativeError::new("regex ParseBy property has an invalid payload"))
}

fn parse_plan(
    metadata: crate::ValueRef<'_>,
    property_type: crate::TypeId,
) -> Result<ParsePlan, NativeError> {
    if let Some(compiled) = attached_regex(metadata, property_type)? {
        let fields = validate_relation(&compiled, metadata, property_type)?;
        return Ok(ParsePlan::Regex { compiled, fields });
    }
    let metadata = resolve_metadata(metadata)?;
    let kind = metadata.dict_get("kind").and_then(|kind| kind.as_atom());
    match kind.as_ref().map(crate::TextRef::as_str) {
        Some("String") => Ok(ParsePlan::String),
        Some("Int") => Ok(ParsePlan::Int),
        Some("Float") => Ok(ParsePlan::Float),
        _ => Err(NativeError::new("type has no std/string.parse capability")),
    }
}

fn validate_relation(
    compiled: &CompiledRegex,
    metadata: crate::ValueRef<'_>,
    property_type: crate::TypeId,
) -> Result<BTreeMap<String, FieldPlan>, NativeError> {
    let metadata = resolve_metadata(metadata)?;
    if !metadata
        .dict_get("kind")
        .and_then(|kind| kind.as_atom())
        .is_some_and(|kind| kind == "Struct")
    {
        return Err(NativeError::new(
            "std/regex.parse_by requires a struct type",
        ));
    }
    let fields = metadata
        .dict_get("fields")
        .ok_or_else(|| NativeError::new("struct type metadata has no fields"))?;
    let names = fields
        .dict_fields()
        .ok_or_else(|| NativeError::new("struct fields metadata must be a Dict"))?
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
    if names != compiled.captures {
        let missing = names
            .difference(&compiled.captures)
            .cloned()
            .collect::<Vec<_>>();
        let extra = compiled
            .captures
            .difference(&names)
            .cloned()
            .collect::<Vec<_>>();
        return Err(NativeError::new(format!(
            "regex captures must match struct fields; missing captures {missing:?}, extra captures {extra:?}"
        )));
    }
    names
        .into_iter()
        .map(|name| {
            let metadata = fields
                .dict_get(&name)
                .expect("field name came from metadata");
            let (optional, metadata) =
                option_payload(metadata).map_or((false, metadata), |payload| (true, payload));
            let plan = parse_plan(metadata, property_type).map_err(|error| {
                NativeError::new(format!(
                    "regex field {name:?} is not string-parsable: {}",
                    error.message
                ))
            })?;
            let capture_optional = !compiled.required.contains(&name);
            if capture_optional != optional {
                return Err(NativeError::new(format!(
                    "regex capture {name:?} is {}, but its field is {}",
                    if capture_optional {
                        "optional"
                    } else {
                        "required"
                    },
                    if optional { "optional" } else { "required" }
                )));
            }
            Ok((name, FieldPlan { plan, optional }))
        })
        .collect()
}

pub(crate) fn native_prepare(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = regex_type(context)?;
    let compiled = compiled_argument(context, 0, &native_type)?;
    let property_type = context
        .value(context.argument(1)?)?
        .declared_type_id()
        .ok_or_else(|| NativeError::new("std/regex.prepare expects ParseBy Type metadata"))?;
    validate_relation(
        &compiled,
        context.value(context.argument(2)?)?,
        property_type,
    )?;
    context.copy(context.result(), context.argument(0)?)
}

fn set_error(context: &mut CallContext<'_, '_>, message: String) -> Result<(), NativeError> {
    let tag = context.scratch()?;
    context.set_atom(tag, "Err")?;
    let error = context.scratch()?;
    let message_register = context.scratch()?;
    context.set_string(message_register, message)?;
    context.make_dict(
        error,
        &[
            ("message".into(), message_register),
            ("data".into(), context.argument(2)?),
            ("rule".into(), context.argument(0)?),
        ],
    )?;
    context.make_tagged(context.result(), tag, error)
}

fn execute_plan(plan: &ParsePlan, input: &str) -> Result<ParsedValue, String> {
    match plan {
        ParsePlan::String => Ok(ParsedValue::String(input.to_owned())),
        ParsePlan::Int => input
            .parse::<i64>()
            .map_err(|_| "input is not a valid Int".to_owned())
            .map(ParsedValue::Int),
        ParsePlan::Float => {
            let value = input
                .parse::<f64>()
                .map_err(|_| "input is not a valid Float".to_owned())?;
            value
                .is_finite()
                .then_some(ParsedValue::Float(value))
                .ok_or_else(|| "input is not a finite Float".to_owned())
        }
        ParsePlan::Regex { compiled, fields } => {
            let captures = compiled
                .regex
                .captures(input)
                .ok_or_else(|| "input does not match regular expression".to_owned())?;
            let mut values = Vec::with_capacity(fields.len());
            for (name, field) in fields {
                let value = match captures.name(name).map(|capture| capture.as_str()) {
                    Some(text) => {
                        let value = execute_plan(&field.plan, text)
                            .map_err(|message| format!("capture {name:?}: {message}"))?;
                        if field.optional {
                            ParsedValue::Some(Box::new(value))
                        } else {
                            value
                        }
                    }
                    None if field.optional => ParsedValue::None,
                    None => return Err(format!("required capture {name:?} is absent")),
                };
                values.push((name.clone(), value));
            }
            Ok(ParsedValue::Struct(values))
        }
    }
}

pub(crate) fn parse_value(
    metadata: crate::ValueRef<'_>,
    input: &str,
    property_type: crate::TypeId,
) -> Result<ParsedValue, String> {
    let plan = parse_plan(metadata, property_type).map_err(|error| error.message)?;
    execute_plan(&plan, input)
}

fn materialize(
    context: &mut CallContext<'_, '_>,
    value: ParsedValue,
    output: crate::lir::RegisterId,
) -> Result<(), NativeError> {
    match value {
        ParsedValue::String(value) => context.set_string(output, value),
        ParsedValue::Int(value) => context.set_int(output, value),
        ParsedValue::Float(value) => context.set_float(output, value),
        ParsedValue::None => context.set_none(output),
        ParsedValue::Some(value) => {
            let payload = context.scratch()?;
            materialize(context, *value, payload)?;
            let tag = context.scratch()?;
            context.set_atom(tag, "Some")?;
            context.make_tagged(output, tag, payload)
        }
        ParsedValue::Struct(fields) => {
            let mut registers = Vec::with_capacity(fields.len());
            for (name, value) in fields {
                let register = context.scratch()?;
                materialize(context, value, register)?;
                registers.push((name, register));
            }
            context.make_dict(output, &registers)
        }
    }
}

pub(crate) fn native_parse(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let property_type = context
        .value(context.argument(0)?)?
        .declared_type_id()
        .ok_or_else(|| NativeError::new("std/string.parse_with expects ParseBy Type metadata"))?;
    let metadata = context.value(context.argument(1)?)?;
    let input = context
        .value(context.argument(2)?)?
        .as_str()
        .ok_or_else(|| NativeError::new("std/string.parse expects String"))?
        .to_owned();
    let parsed = match parse_value(metadata, &input, property_type) {
        Ok(value) => value,
        Err(message) => return set_error(context, message),
    };
    let value = context.scratch()?;
    materialize(context, parsed, value)?;
    let tag = context.scratch()?;
    context.set_atom(tag, "Ok")?;
    context.make_tagged(context.result(), tag, value)
}

#[cfg(test)]
#[path = "regex/tests/mod.rs"]
mod tests;
