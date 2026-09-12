//! Narrow C ABI used only by our generated code. Never called with host Val.
use super::*;
use crate::abi::{CallContext, Origin, Status};

pub(crate) const STRING: u32 = 0;
pub(crate) const AGGREGATE: u32 = 1;
pub(crate) const ARRAY: u32 = 2;
pub(crate) const FIELD: u32 = 3;
pub(crate) const INDEX: u32 = 4;
pub(crate) const DICT: u32 = 5;
pub(crate) const FAIL: u32 = 6;
pub(crate) const ENUM: u32 = 7;
pub(crate) const PAYLOAD: u32 = 8;
pub(crate) const TEXT_EQUAL: u32 = 9;
pub(crate) const CLOSURE: u32 = 10;
pub(crate) const CAPTURE: u32 = 11;
pub(crate) const DEMAND: u32 = 12;
pub(crate) const ARRAY_LENGTH: u32 = 13;
pub(crate) const STRING_LENGTH: u32 = 14;
pub(crate) const ARRAY_MAP: u32 = 15;
pub(crate) const PROPERTY_MARK: u32 = 16;
pub(crate) const DYN_PACK: u32 = 17;
pub(crate) const DYN_DESC: u32 = 18;
pub(crate) const DYN_PROJECT: u32 = 19;
pub(crate) const DYN_CHECK: u32 = 20;
pub(crate) const DYN_KIND: u32 = 21;
pub(crate) const DYN_FIELD: u32 = 22;
pub(crate) const DYN_QUERY: u32 = 23;
pub(crate) const DYN_MEMBER: u32 = 24;
pub(crate) const FORMAT: u32 = 25;
pub(crate) const FAIL_VALUES: u32 = 26;
pub(crate) const REGEX: u32 = 27;
pub(crate) const TEXT_OP: u32 = 28;
pub(crate) const REFLECT: u32 = 29;
pub(crate) const ENCODE: u32 = 30;
pub(crate) const DECODE: u32 = 42;
pub(crate) const CHECK_RESULT: u32 = 43;
pub(crate) const PARSE: u32 = 44;
pub(crate) const FORMAT_PARSE: u32 = 45;
pub(crate) const JSON_STRINGIFY: u32 = 46;
pub(crate) const JSON_INDENT: u32 = 47;
pub(crate) const JSON_SCHEMA: u32 = 48;
pub(crate) const PATH: u32 = 49;
pub(crate) const HASH: u32 = 50;
pub(crate) const BYTES_EQUAL: u32 = 51;
pub(crate) const DIAGNOSTIC_SCOPE: u32 = 52;
pub(crate) const EQUAL: u32 = 53;
pub(crate) const FLOAT_REMAINDER: u32 = 54;
pub(crate) const MAKE_TEST: u32 = 55;
pub(crate) const ARRAY_CONCAT: u32 = 56;
pub(crate) const CHECKED_CAST: u32 = 57;
pub(crate) const BYTES_LITERAL: u32 = 58;
pub(crate) const INTERPOLATE: u32 = 31;
pub(crate) const FUEL: u32 = 32;
pub(crate) const ENTER_CALL: u32 = 33;
pub(crate) const LEAVE_CALL: u32 = 34;
pub(crate) const DICT_READ: u32 = 35;
pub(crate) const FOLD: u32 = 36;
pub(crate) const ARRAY_GET: u32 = 37;
pub(crate) const ARRAY_BUILD: u32 = 38;
pub(crate) const BLAME: u32 = 39;
pub(crate) const RAISE: u32 = 40;
pub(crate) const WARN: u32 = 41;
#[path = "callbacks.rs"]
mod callbacks;

/// Safety: ctx is an exclusive live context; data points at the full values
/// specified by the generated operation; out has space for the solved result.
/// No pointer is retained and no host panic unwinds across this boundary.
pub(crate) unsafe extern "C" fn object(
    ctx: *mut CallContext,
    operation: u32,
    ty: u32,
    loc0: u64,
    end: u32,
    data: *const u64,
    count: u64,
    out: *mut u64,
) -> u32 {
    // SAFETY: guaranteed by the private codegen ABI above.
    let context = unsafe { &mut *ctx };
    let origin = match Origin::from_words([loc0 as u32, (loc0 >> 32) as u32, end]) {
        Ok(origin) => origin,
        Err(e) => return context.fail(e) as u32,
    };
    if operation == ENTER_CALL { return context.enter_frame(count, origin) as u32; }
    if operation == DIAGNOSTIC_SCOPE { return unsafe { callbacks::diagnostic_scope(context, TypeId(ty), data, out, origin) }; }
    if operation == CHECKED_CAST { return unsafe { callbacks::checked_cast(context, TypeId(ty), data, out, origin, count) }; }
    if operation == LEAVE_CALL { return context.leave_call() as u32; }
    if operation == FUEL {
        return context.consume_fuel(count, origin) as u32;
    }
    if operation == DEMAND {
        // The data argument is a code address for this operation only. It is
        // emitted by func_addr and never stored in a language value or runtime.
        return unsafe { demand(context, TypeId(ty), count, data, out, origin) };
    }
    if operation == ARRAY_MAP {
        return unsafe { callbacks::array_map(context, TypeId(ty), data, out, origin, count) };
    }
    if operation == DECODE || operation == ENCODE || operation == PARSE || operation == JSON_SCHEMA {
        return unsafe { callbacks::codec(context, TypeId(ty), data, out, origin, count, operation) };
    }
    if operation == FORMAT_PARSE {
        return unsafe { callbacks::format_parse(context, TypeId(ty), data, out, origin, count) };
    }
    if operation == CHECK_RESULT {
        return context.boundary(|context| {
            let outcome = (|| -> Result<Option<(String, Vec<Origin>)>> {
                let rt = context.runtime()?;
                let ty = TypeId(ty);
                let width = rt.layout(ty)?.words;
                let value = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data, width) }.into() };
                let tag = rt.enum_tag(&value)? as usize;
                match rt.layout(ty)?.variants[tag].name.as_str() {
                    "Ok" => {
                        unsafe { std::ptr::copy_nonoverlapping(value.words().as_ptr(), out, width); }
                        Ok(None)
                    }
                    "Err" => {
                        let blame = rt.enum_payload(&value)?.ok_or("checker error has no blame")?.to_owned();
                        Ok(Some(rt.blame_diagnostic(&blame)?))
                    }
                    _ => Err("checker result is not Result".into()),
                }
            })();
            match outcome {
                Ok(None) => Status::Success,
                Ok(Some((message, subjects))) => context.fail_with_subjects(message, origin, subjects),
                Err(error) => context.fail_at(error, origin),
            }
        }) as u32;
    }
    if operation == FOLD {
        return unsafe { callbacks::fold(context, TypeId(ty), data, out, origin, count) };
    }
    if operation == FAIL_VALUES || operation == RAISE || operation == WARN {
        return context.boundary(|context| {
            let result = (|| -> Result<(String, Vec<Origin>)> {
                let rt = context.runtime()?;
                let count = usize::try_from(count).map_err(|_| "diagnostic packet overflow")?;
                let mut words = unsafe { std::slice::from_raw_parts(data, count) };
                let mut message = None;
                let mut subjects = Vec::new();
                let mut from_blame = false;
                while !words.is_empty() {
                    let ty =
                        TypeId((words.get(1).ok_or("truncated diagnostic value")? >> 32) as u32);
                    let width = rt.layout(ty)?.words;
                    let value = ValueRef {
                        arena: rt.identity,
                        words: words.get(..width).ok_or("truncated diagnostic value")?,
                    };
                    rt.validate(value, ty)?;
                    if message.is_none() {
                        if operation != FAIL_VALUES && rt.layout(ty)?.kind == Kind::Blame {
                            let (text, stored) = rt.blame_diagnostic(&value.to_owned())?;
                            message = Some(text);
                            subjects = stored;
                            from_blame = true;
                        } else { message = Some(rt.text(value)?.as_str().to_owned()); }
                    } else if !from_blame {
                        let origin = Origin::from_words(value.location())?;
                        if origin.words()[0] != 0 && !subjects.contains(&origin) {
                            subjects.push(origin);
                        }
                    }
                    words = &words[width..];
                }
                Ok((message.ok_or("diagnostic message missing")?, subjects))
            })();
            match result {
                Ok((message, subjects)) if operation == WARN => {
                    let result = context.runtime_mut().and_then(|rt| rt.named_variant(TypeId(ty), origin.words(), "None", None));
                    match result {
                        Ok(value) => {
                            unsafe { std::ptr::copy_nonoverlapping(value.words().as_ptr(), out, value.words().len()); }
                            context.warn(message, origin, subjects);
                            Status::Success
                        }
                        Err(message) => context.fail_at(message, origin),
                    }
                }
                Ok((message, subjects)) => context.fail_with_subjects(message, origin, subjects),
                Err(message) => context.fail_at(message, origin),
            }
        }) as u32;
    }
    context.boundary(|context| {
        let result = (|| {
            if operation == FAIL {
                // Static diagnostic bytes owned by the JIT module.
                let message =
                    unsafe { std::slice::from_raw_parts(data.cast::<u8>(), count as usize) };
                return Err(String::from_utf8_lossy(message).into_owned());
            }
            let rt = context.runtime_mut()?;
            let ty = TypeId(ty);
            let loc = origin.words();
            let count = usize::try_from(count).map_err(|_| "native count overflow")?;
            let result = match operation {
                BLAME => {
                    let mut words = unsafe { std::slice::from_raw_parts(data, count) };
                    let mut message = None;
                    let mut subjects = Vec::new();
                    while !words.is_empty() {
                        let input = TypeId((words.get(1).ok_or("truncated blame packet")? >> 32) as u32);
                        let width = rt.layout(input)?.words;
                        let value = ValueRef { arena: rt.identity, words: words.get(..width).ok_or("truncated blame value")? };
                        rt.validate(value, input)?;
                        if message.is_none() { message = Some(value.to_owned()); }
                        else { subjects.push(Origin::from_words(value.location())?); }
                        words = &words[width..];
                    }
                    rt.blame(ty, loc, &message.ok_or("blame message missing")?, subjects)?
                }
                ARRAY_CONCAT => {
                    let words = unsafe { std::slice::from_raw_parts(data, count.checked_mul(4).ok_or("array spread packet overflow")?) };
                    let arrays = words.chunks_exact(4).map(|words| Value { arena: rt.identity, words: words.into() }).collect::<Vec<_>>();
                    rt.array_concat(ty, loc, &arrays)?
                }
                ARRAY_BUILD => {
                    let mut inputs = Vec::new();
                    let mut cursor = data;
                    for _ in 0..if count == 1 || count == 3 { 2 } else { 1 } {
                        let input = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                        let width = rt.layout(input)?.words;
                        inputs.push(Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(cursor, width) }.into() });
                        cursor = unsafe { cursor.add(width) };
                    }
                    rt.array_operation(ty, loc, count, &inputs)?
                }
                ARRAY_GET => {
                    let array = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data, 4) }.into() };
                    let index = ValueRef { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data.add(4), 3) } };
                    let index = rt.scalar_bits(index)? as i64;
                    let value = if index >= 0 && (index as u64) < rt.array_len(&array)? as u64 {
                        Some(rt.array_get(&array, index as usize)?.to_owned())
                    } else { None };
                    rt.named_variant(ty, loc, if value.is_some() { "Some" } else { "None" }, value.as_ref())?
                }
                DICT_READ => {
                    let input = unsafe { TypeId((*data.add(1) >> 32) as u32) };
                    let width = rt.layout(input)?.words;
                    let dict = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data, width) }.into() };
                    if count == 0 {
                        let key = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data.add(width), 4) }.into() };
                        let value = rt.dict_get(&dict, &key)?.map(ValueRef::to_owned);
                        rt.named_variant(ty, loc, if value.is_some() { "Some" } else { "None" }, value.as_ref())?
                    } else if count == 3 {
                        rt.dict_pairs(ty, loc, &dict)?
                    } else if count == 4 {
                        rt.dict_from_pairs(ty, loc, &dict)?
                    } else if count == 5 {
                        let right = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data.add(width), width) }.into() };
                        rt.dict_merge(ty, loc, &dict, &right)?
                    } else {
                        rt.dict_column(ty, loc, &dict, count == 1)?
                    }
                }
                MAKE_TEST => {
                    let mut inputs = vec![];
                    let mut cursor = data;
                    for _ in 0..if count < 2 { 1 } else { 2 } {
                        let input = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                        let width = rt.layout(input)?.words;
                        inputs.push(Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(cursor, width) }.into() });
                        cursor = unsafe { cursor.add(width) };
                    }
                    rt.make_test(ty, loc, count, &inputs)?
                }
                FLOAT_REMAINDER => {
                    let values = unsafe { std::slice::from_raw_parts(data, 6) };
                    let result = f64::from_bits(values[2]) % f64::from_bits(values[5]);
                    if !result.is_finite() { return Err("floating-point arithmetic produced a non-finite result".into()); }
                    rt.scalar(ty, loc, result.to_bits())?
                }
                EQUAL => {
                    let input = unsafe { TypeId((*data.add(1) >> 32) as u32) };
                    let width = rt.layout(input)?.words;
                    let left = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data, width) }.into() };
                    let right = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data.add(width), width) }.into() };
                    let equal = rt.equal(&left, &right)?;
                    rt.scalar(ty, loc, u64::from(equal))?
                }
                HASH => {
                    let mut inputs = Vec::new();
                    let mut cursor = data;
                    for _ in 0..match count { 1 => 0, 2..=4 => 2, _ => 1 } {
                        let input = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                        let width = rt.layout(input)?.words;
                        inputs.push(Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(cursor, width) }.into() });
                        cursor = unsafe { cursor.add(width) };
                    }
                    rt.hash(ty, &inputs, loc, count)?
                }
                PATH => {
                    let input = unsafe { TypeId((*data.add(1) >> 32) as u32) };
                    let value = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data, rt.layout(input)?.words) }.into() };
                    rt.path(ty, &value, loc, count)?
                }
                JSON_INDENT => {
                    let value = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data, 3) }.into() };
                    let indent = rt.scalar_bits(value.as_ref())? as i64;
                    if !(0..=16).contains(&indent) { return Err("std/json.stringify_pretty indent must be between 0 and 16".into()); }
                    value
                }
                JSON_STRINGIFY => {
                    let input = unsafe { TypeId((*data.add(1) >> 32) as u32) };
                    let value = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data, rt.layout(input)?.words) }.into() };
                    let contract = rt.data_contract.clone().ok_or("semantic Value contract is not loaded")?;
                    let indent = if count == 0 { None } else { Some((count - 1) as usize) };
                    let text = rt.semantic_json_indented(&contract, &value, indent)?;
                    rt.owned_string(ty, loc, text)?
                }
                INTERPOLATE => {
                    let mut cursor = data;
                    let mut text = String::new();
                    for _ in 0..count {
                        let input = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                        let width = rt.layout(input)?.words;
                        let value = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(cursor, width) }.into() };
                        match rt.layout(input)?.kind {
                            Kind::String => text.push_str(rt.text(value.as_ref())?.as_str()),
                            Kind::Format => text.push_str(&rt.format_render(&value)?),
                            _ => return Err("invalid sealed interpolation part".into()),
                        }
                        cursor = unsafe { cursor.add(width) };
                    }
                    rt.owned_string(ty, loc, text)?
                }
                REFLECT => {
                    let input = unsafe { TypeId((*data.add(1) >> 32) as u32) };
                    let width = rt.layout(input)?.words;
                    let value = Value {
                        arena: rt.identity,
                        words: unsafe { std::slice::from_raw_parts(data, width) }.into(),
                    };
                    rt.reflect(ty, count, &value)?
                }
                TEXT_OP => {
                    let arity = match count {
                        1 | 3 | 9 => 1,
                        7 => 3,
                        _ => 2,
                    };
                    let mut inputs = Vec::new();
                    let mut cursor = data;
                    for _ in 0..arity {
                        let input = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                        let width = rt.layout(input)?.words;
                        inputs.push(Value {
                            arena: rt.identity,
                            words: unsafe { std::slice::from_raw_parts(cursor, width) }.into(),
                        });
                        cursor = unsafe { cursor.add(width) };
                    }
                    rt.text_operation(ty, loc, count, &inputs)?
                }
                REGEX => {
                    let arity = match count {
                        0 => 1,
                        1 => 2,
                        2 => 3,
                        _ => return Err("unknown Regex operation".into()),
                    };
                    let mut inputs = Vec::new();
                    let mut cursor = data;
                    for _ in 0..arity {
                        let input = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                        let width = rt.layout(input)?.words;
                        inputs.push(Value {
                            arena: rt.identity,
                            words: unsafe { std::slice::from_raw_parts(cursor, width) }.into(),
                        });
                        cursor = unsafe { cursor.add(width) };
                    }
                    match count {
                        0 => rt.regex_compile(ty, loc, &inputs[0])?,
                        1 => rt.scalar(
                            ty,
                            loc,
                            u64::from(rt.regex_matches(&inputs[0], &inputs[1])?),
                        )?,
                        _ => rt.regex_prepare(&inputs[0], &inputs[1], &inputs[2])?,
                    }
                }
                FORMAT => {
                    let mut inputs = Vec::new();
                    let mut cursor = data;
                    for _ in 0..if count == 4 { 2 } else { 1 } {
                        let input = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                        let width = rt.layout(input)?.words;
                        inputs.push(Value {
                            arena: rt.identity,
                            words: unsafe { std::slice::from_raw_parts(cursor, width) }.into(),
                        });
                        cursor = unsafe { cursor.add(width) };
                    }
                    match count {
                        0 => rt.format_prepare(ty, loc, &inputs[0])?,
                        1..=4 => rt.format_node(ty, loc, count as u64, &inputs)?,
                        5 => {
                            let text = rt.format_render(&inputs[0])?;
                            rt.string(ty, loc, &text)?
                        }
                        _ => return Err("unknown format operation".into()),
                    }
                }
                DYN_PACK | DYN_DESC | DYN_PROJECT | DYN_CHECK | DYN_KIND | DYN_FIELD
                | DYN_QUERY | DYN_MEMBER => {
                    let read = |pointer: *const u64| -> Result<Value> {
                        let input = unsafe { TypeId((*pointer.add(1) >> 32) as u32) };
                        Ok(Value {
                            arena: rt.identity,
                            words: unsafe {
                                std::slice::from_raw_parts(pointer, rt.layout(input)?.words)
                            }
                            .into(),
                        })
                    };
                    let first = read(data)?;
                    if operation == DYN_MEMBER {
                        let index = if count == 1 {
                            None
                        } else {
                            let index = read(unsafe { data.add(first.words.len()) })?;
                            Some(
                                u32::try_from(rt.scalar_bits(index.as_ref())?)
                                    .map_err(|_| "Dyn member index must be a non-negative u32")?,
                            )
                        };
                        rt.dynamic_member(ty, loc, &first, count, index)?
                    } else if operation == DYN_QUERY {
                        let query = match count {
                            0 => DynamicQuery::Fields,
                            1 => DynamicQuery::ArrayItems,
                            2 => DynamicQuery::TupleItems,
                            3 => DynamicQuery::Tag,
                            4 => DynamicQuery::Payload,
                            _ => return Err("unknown Dyn query".into()),
                        };
                        rt.dynamic_query(ty, loc, &first, query)?
                    } else if operation == DYN_FIELD {
                        let name = read(unsafe { data.add(first.words.len()) })?;
                        rt.dynamic_value(&first)?;
                        let outcome = rt.dynamic_field(&first, &name).map(ValueRef::to_owned);
                        let arguments = rt.layout(ty)?.arguments.clone();
                        let (tag, value) = match outcome {
                            Ok(value) => {
                                ("Ok", rt.dynamic(arguments[0], value.location(), &value)?)
                            }
                            Err(message) => ("Err", rt.string(arguments[1], loc, &message)?),
                        };
                        let index = rt
                            .layout(ty)?
                            .variants
                            .iter()
                            .position(|v| v.name == tag)
                            .ok_or("Dyn field Result variant missing")?;
                        rt.enum_value(ty, loc, index as u32, Some(&value))?
                    } else if operation == DYN_KIND {
                        let name = rt.dynamic_kind(&first)?;
                        let tag = rt
                            .layout(ty)?
                            .variants
                            .iter()
                            .position(|v| v.name == name && v.payload.is_none())
                            .ok_or("Dyn kind result ABI mismatch")?;
                        rt.enum_value(ty, loc, tag as u32, None)?
                    } else if operation == DYN_PACK {
                        let value = read(unsafe { data.add(first.words.len()) })?;
                        if rt.represented_type(first.as_ref())? != value.type_id() {
                            return Err("Dyn packing witness mismatch".into());
                        }
                        rt.dynamic(ty, loc, &value)?
                    } else {
                        let dynamic = if operation == DYN_PROJECT {
                            read(unsafe { data.add(first.words.len()) })?
                        } else {
                            first.clone()
                        };
                        let value = rt.dynamic_value(&dynamic)?.to_owned();
                        if operation == DYN_DESC {
                            rt.metadata(ty, loc, value.type_id())?
                        } else {
                            let expected = if operation == DYN_CHECK {
                                *rt.layout(ty)?
                                    .arguments
                                    .first()
                                    .ok_or("Dyn check Option type missing")?
                            } else {
                                rt.represented_type(first.as_ref())?
                            };
                            let some = rt
                                .layout(ty)?
                                .variants
                                .iter()
                                .position(|v| v.name == "Some" && v.payload == Some(expected))
                                .ok_or("Dyn projection Option signature mismatch")?;
                            if value.type_id() == expected {
                                rt.enum_value(ty, loc, some as u32, Some(&value))?
                            } else {
                                let none = rt
                                    .layout(ty)?
                                    .variants
                                    .iter()
                                    .position(|v| v.name == "None" && v.payload.is_none())
                                    .ok_or("Dyn projection None missing")?;
                                rt.enum_value(ty, loc, none as u32, None)?
                            }
                        }
                    }
                }
                PROPERTY_MARK => {
                    let target = Value {
                        arena: rt.identity,
                        words: unsafe { std::slice::from_raw_parts(data, 3) }.into(),
                    };
                    let masks = [4u64, 16, 8, 2, 1, 32];
                    let mut bits = *masks
                        .get(rt.enum_tag(&target)? as usize)
                        .ok_or("invalid PropertyTarget tag")?;
                    let previous_ty = unsafe { TypeId((*data.add(4) >> 32) as u32) };
                    let previous = Value {
                        arena: rt.identity,
                        words: unsafe {
                            std::slice::from_raw_parts(data.add(3), rt.layout(previous_ty)?.words)
                        }
                        .into(),
                    };
                    if let Some(previous) = rt.enum_payload(&previous)? {
                        let previous = previous.to_owned();
                        rt.validate(previous.as_ref(), ty)?;
                        bits |= rt.scalar_bits(rt.field(&previous, 0)?)?;
                    }
                    let bit_type = rt
                        .layout(ty)?
                        .fields
                        .first()
                        .ok_or("property attribute missing bits")?
                        .0;
                    let bits = rt.scalar(bit_type, loc, bits)?;
                    rt.aggregate(ty, loc, &[bits])?
                }
                CLOSURE => {
                    let words = unsafe { std::slice::from_raw_parts(data, count) };
                    let function = u32::try_from(*words.first().ok_or("closure function missing")?)
                        .map_err(|_| "function ID overflow")?;
                    let mut remaining = &words[1..];
                    let mut captures = Vec::new();
                    while !remaining.is_empty() {
                        let header = remaining.get(1).ok_or("truncated capture header")?;
                        let size = rt.layout(TypeId((header >> 32) as u32))?.words;
                        let words = remaining.get(..size).ok_or("truncated capture value")?;
                        captures.push(Value {
                            arena: rt.identity,
                            words: words.to_vec().into_boxed_slice(),
                        });
                        remaining = &remaining[size..];
                    }
                    rt.closure(ty, loc, function, &captures)?
                }
                ENUM => {
                    let index = u32::try_from(count).map_err(|_| "enum tag overflow")?;
                    let payload_ty = rt
                        .expect(ty, Kind::Enum)?
                        .variants
                        .get(count)
                        .ok_or("enum tag out of bounds")?
                        .payload;
                    let payload = match payload_ty {
                        Some(payload_ty) => {
                            let words = unsafe {
                                std::slice::from_raw_parts(data, rt.layout(payload_ty)?.words)
                            };
                            Some(Value {
                                arena: rt.identity,
                                words: words.to_vec().into_boxed_slice(),
                            })
                        }
                        None => None,
                    };
                    rt.enum_value(ty, loc, index, payload.as_ref())?
                }
                BYTES_LITERAL => {
                    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), count as usize) };
                    rt.bytes(ty, loc, bytes)?
                }
                STRING => {
                    // SAFETY: codegen stores exactly count bytes in its JIT data object.
                    let bytes = unsafe { std::slice::from_raw_parts(data.cast::<u8>(), count) };
                    let text =
                        std::str::from_utf8(bytes).map_err(|_| "native string is not UTF-8")?;
                    rt.string(ty, loc, text)?
                }
                AGGREGATE | ARRAY | DICT => {
                    let types = match operation {
                        AGGREGATE => rt
                            .layout(ty)?
                            .fields
                            .iter()
                            .map(|(t, _)| *t)
                            .collect::<Vec<_>>(),
                        ARRAY => vec![rt.expect(ty, Kind::Array)?.arguments[0]; count],
                        DICT => {
                            let element = rt.expect(ty, Kind::Dict)?.arguments[0];
                            let mut types = Vec::with_capacity(
                                count
                                    .checked_mul(2)
                                    .ok_or("native dictionary count overflow")?,
                            );
                            let mut cursor = data;
                            for _ in 0..count {
                                // String's solved full width is four words.
                                let key = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                                rt.expect(key, Kind::String)?;
                                types.extend([key, element]);
                                cursor = unsafe { cursor.add(4 + rt.layout(element)?.words) };
                            }
                            types
                        }
                        _ => unreachable!(),
                    };
                    if operation == AGGREGATE && types.len() != count {
                        return Err("native aggregate arity mismatch".into());
                    }
                    let mut cursor = data;
                    let mut values = Vec::with_capacity(types.len());
                    for t in types {
                        let size = rt.layout(t)?.words;
                        let words = unsafe { std::slice::from_raw_parts(cursor, size) };
                        let value = Value {
                            arena: rt.identity,
                            words: words.to_vec().into_boxed_slice(),
                        };
                        rt.validate(value.as_ref(), t)?;
                        values.push(value);
                        cursor = unsafe { cursor.add(size) };
                    }
                    match operation {
                        AGGREGATE => rt.aggregate(ty, loc, &values)?,
                        ARRAY => rt.array(ty, loc, &values)?,
                        DICT => {
                            let pairs = values
                                .chunks_exact(2)
                                .map(|p| (p[0].clone(), p[1].clone()))
                                .collect::<Vec<_>>();
                            rt.dict(ty, loc, &pairs)?
                        }
                        _ => unreachable!(),
                    }
                }
                FIELD | INDEX | PAYLOAD | TEXT_EQUAL | BYTES_EQUAL | CAPTURE | ARRAY_LENGTH | STRING_LENGTH => {
                    let receiver_ty = unsafe { TypeId((*data.add(1) >> 32) as u32) };
                    let size = rt.layout(receiver_ty)?.words;
                    let words = unsafe { std::slice::from_raw_parts(data, size) };
                    let receiver = Value {
                        arena: rt.identity,
                        words: words.to_vec().into_boxed_slice(),
                    };
                    if operation == ARRAY_LENGTH || operation == STRING_LENGTH {
                        let length = if operation == ARRAY_LENGTH {
                            rt.array_len(&receiver)?
                        } else {
                            rt.text(receiver.as_ref())?.as_str().chars().count()
                        };
                        rt.scalar(
                            ty,
                            loc,
                            i64::try_from(length).map_err(|_| "native length overflow")? as u64,
                        )?
                    } else if operation == CAPTURE {
                        rt.capture(&receiver, count)?.to_owned()
                    } else if operation == PAYLOAD {
                        rt.enum_payload(&receiver)?
                            .ok_or("enum has no payload")?
                            .to_owned()
                    } else if operation == BYTES_EQUAL {
                        let other = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data.add(size), size) }.into() };
                        let equal = rt.bytes_data(&receiver)? == rt.bytes_data(&other)?;
                        rt.scalar(ty, loc, u64::from(equal))?
                    } else if operation == TEXT_EQUAL {
                        rt.expect(receiver_ty, Kind::String)?;
                        let other = unsafe { std::slice::from_raw_parts(data.add(size), 4) };
                        let other = ValueRef {
                            arena: rt.identity,
                            words: other,
                        };
                        let equal =
                            rt.text(receiver.as_ref())?.as_str() == rt.text(other)?.as_str();
                        rt.scalar(ty, loc, u64::from(equal))?
                    } else if operation == FIELD {
                        rt.field(&receiver, count)?.to_owned()
                    } else {
                        rt.array_get(&receiver, count)?.to_owned()
                    }
                }
                _ => return Err("unknown native object operation".into()),
            };
            rt.validate(result.as_ref(), ty)?;
            // SAFETY: result width checked against the codegen's solved TypeId.
            unsafe {
                std::ptr::copy_nonoverlapping(result.words.as_ptr(), out, result.words.len());
            }
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => Status::Success,
            Err(error) => context.fail_at(error, origin),
        }
    }) as u32
}

pub(crate) unsafe fn demand(
    context: &mut CallContext,
    ty: TypeId,
    slot: u64,
    address: *const u64,
    out: *mut u64,
    origin: Origin,
) -> u32 {
    context.boundary(|context| {
        let result = (|| {
            let rt = context.runtime_mut()?;
            let key = *rt
                .demand_keys
                .get(usize::try_from(slot).map_err(|_| "native demand index overflow")?)
                .ok_or("native demand slot outside plan")?;
            if rt.demands.get(&key).ok_or("native demand missing")?.ty != ty {
                return Err("native demand type mismatch".into());
            }
            let width = rt.layout(ty)?.words;
            let value = match rt.begin_demand(key)? {
                Demand::Failed => return Ok(Status::Failed),
                Demand::Ready(value) => value,
                Demand::Evaluate => {
                    let mut words = vec![0; width].into_boxed_slice();
                    type Initializer = unsafe extern "C" fn(
                        *mut CallContext,
                        *const u64,
                        *mut u64,
                        *const u64,
                    ) -> u32;
                    // SAFETY: codegen passes a no-argument initializer of the
                    // checked result type, alive for this whole borrowed call.
                    // No Runtime borrow remains live across reentry.
                    let initializer =
                        unsafe { std::mem::transmute::<*const u64, Initializer>(address) };
                    let status = unsafe {
                        initializer(
                            context,
                            std::ptr::null(),
                            words.as_mut_ptr(),
                            std::ptr::null(),
                        )
                    };
                    if status != 0 {
                        context.runtime_mut()?.fail_demand(key)?;
                        if status != 1 {
                            return Err("native initializer returned invalid status".into());
                        }
                        return Ok(Status::Failed);
                    }
                    let rt = context.runtime_mut()?;
                    let value = Value {
                        arena: rt.identity,
                        words,
                    };
                    if let Err(error) = rt.complete_demand(key, value.clone()) {
                        rt.fail_demand(key)?;
                        return Err(error);
                    }
                    value
                }
            };
            // SAFETY: the generated output buffer has the validated full width.
            unsafe {
                std::ptr::copy_nonoverlapping(value.words().as_ptr(), out, width);
            }
            Ok::<Status, String>(Status::Success)
        })();
        match result {
            Ok(status) => status,
            Err(error) => context.fail_at(error, origin),
        }
    }) as u32
}
