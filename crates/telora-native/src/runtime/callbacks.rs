use super::*;

type Callback = unsafe extern "C" fn(*mut CallContext, *const u64, *mut u64, *const u64) -> u32;

/// All addresses and descriptor buffers are borrowed from the current native
/// call. Runtime borrows end before calling generated code recursively.
pub(super) unsafe fn array_map(
    context: &mut CallContext,
    ty: TypeId,
    data: *const u64,
    out: *mut u64,
    origin: Origin,
) -> u32 {
    context.boundary(|context| {
        let result = (|| {
            let rt = context.runtime()?;
            let element = rt.expect(ty, Kind::Array)?.arguments[0];
            let output_width = rt.layout(element)?.words;
            // SAFETY: codegen constructs this packet after checking the exact
            // array/function layouts and callback parameter/result types.
            let address = unsafe { *data };
            let array = Value {
                arena: rt.identity,
                words: unsafe { std::slice::from_raw_parts(data.add(1), 4) }.into(),
            };
            let closure = Value {
                arena: rt.identity,
                words: unsafe { std::slice::from_raw_parts(data.add(5), 3) }.into(),
            };
            rt.function_id(&closure)?;
            let count = rt.array_len(&array)?;
            let callback = unsafe { std::mem::transmute::<usize, Callback>(address as usize) };
            let mut mapped = Vec::with_capacity(count);
            for index in 0..count {
                let argument = context.runtime()?.array_get(&array, index)?.to_owned();
                let mut words = vec![0; output_width].into_boxed_slice();
                let status = unsafe {
                    callback(
                        context,
                        argument.words().as_ptr(),
                        words.as_mut_ptr(),
                        closure.words().as_ptr(),
                    )
                };
                if status == 1 {
                    return Ok(Status::Failed);
                }
                if status != 0 {
                    return Err("native callback returned invalid status".into());
                }
                let rt = context.runtime()?;
                let value = Value {
                    arena: rt.identity,
                    words,
                };
                rt.validate(value.as_ref(), element)?;
                mapped.push(value);
            }
            let value = context.runtime_mut()?.array(ty, origin.words(), &mapped)?;
            // SAFETY: output size is the checked Array result layout.
            unsafe {
                std::ptr::copy_nonoverlapping(value.words().as_ptr(), out, value.words().len());
            }
            Ok::<Status, String>(Status::Success)
        })();
        match result {
            Ok(status) => status,
            Err(error) => context.fail_at(error, origin),
        }
    }) as u32
}
