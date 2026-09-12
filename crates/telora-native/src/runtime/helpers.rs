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
    if operation == DEMAND {
        // The data argument is a code address for this operation only. It is
        // emitted by func_addr and never stored in a language value or runtime.
        return unsafe { demand(context, TypeId(ty), count, data, out, origin) };
    }
    if operation == ARRAY_MAP {
        return unsafe { callbacks::array_map(context, TypeId(ty), data, out, origin) };
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
                DYN_PACK | DYN_DESC | DYN_PROJECT | DYN_CHECK | DYN_KIND | DYN_FIELD => {
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
                    if operation == DYN_FIELD {
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
                FIELD | INDEX | PAYLOAD | TEXT_EQUAL | CAPTURE | ARRAY_LENGTH | STRING_LENGTH => {
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
