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
                FIELD | INDEX => {
                    let receiver_ty = unsafe { TypeId((*data.add(1) >> 32) as u32) };
                    let size = rt.layout(receiver_ty)?.words;
                    let words = unsafe { std::slice::from_raw_parts(data, size) };
                    let receiver = Value {
                        arena: rt.identity,
                        words: words.to_vec().into_boxed_slice(),
                    };
                    if operation == FIELD {
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
