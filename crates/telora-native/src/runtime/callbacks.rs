use super::*;

type Callback = unsafe extern "C" fn(*mut CallContext, *const u64, *mut u64, *const u64) -> u32;

pub(super) unsafe fn checked_cast(context: &mut CallContext, ty: TypeId, data: *const u64, out: *mut u64, origin: Origin, count: u64) -> u32 {
    context.boundary(|context| {
        let result = (|| -> Result<Status> {
            if count >> 32 != 0 { return Err("cast packet must not contain property providers".into()); }
            let rt = context.runtime()?;
            let cursor = unsafe { *data } as *const u64;
            let input_type = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
            let input = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(cursor, rt.layout(input_type)?.words) }.into() };
            let mut checks = vec![];
            for index in 0..count as usize {
                let words = unsafe { std::slice::from_raw_parts(data.add(1 + index * 6), 6) };
                checks.push(codec::CheckPlan { owner: TypeId(words[0] as u32), site: words[1], slot: words[2], signature: TypeId(words[3] as u32), initializer: words[4] as usize, dispatcher: words[5] as usize });
            }
            let mut codec = codec::Codec { context, checks: &checks, properties: &[] };
            let Some(value) = codec.checked_cast(ty, &input, origin)? else { return Ok(Status::Failed); };
            context.runtime()?.validate(value.as_ref(), ty)?;
            unsafe { std::ptr::copy_nonoverlapping(value.words().as_ptr(), out, value.words().len()); }
            Ok(Status::Success)
        })();
        match result { Ok(status) => status, Err(message) => context.fail_at(message, origin) }
    }) as u32
}

pub(super) unsafe fn diagnostic_scope(context: &mut CallContext, ty: TypeId, data: *const u64, out: *mut u64, origin: Origin) -> u32 {
    context.boundary(|context| {
        let result = (|| -> Result<Status> {
            let rt = context.runtime()?;
            let address = unsafe { *data };
            let mut cursor = unsafe { data.add(1) };
            let mut inputs = vec![];
            for _ in 0..6 {
                let input = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                let width = rt.layout(input)?.words;
                inputs.push(Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(cursor, width) }.into() });
                cursor = unsafe { cursor.add(width) };
            }
            let returned = *rt.layout(inputs[0].type_id())?.arguments.last().ok_or("diagnostic callback lacks output")?;
            let never = rt.type_info[returned.index()].kind == Some("Never");
            let mut output = vec![0; if never { 0 } else { rt.layout(returned)?.words }];
            let reports_ty = rt.layout(ty)?.arguments[1];
            let diagnostic_ty = rt.layout(reports_ty)?.arguments[0];
            if rt.represented_type(inputs[2].as_ref())? != diagnostic_ty { return Err("diagnostic witness mismatch".into()); }
            let severity = rt.diagnostic_field_type(diagnostic_ty, "severity")?;
            let labels = rt.diagnostic_field_type(diagnostic_ty, "labels")?;
            let label = rt.layout(labels)?.arguments[0];
            let range = rt.diagnostic_field_type(label, "location")?;
            for (input, expected) in inputs[3..].iter().zip([severity, label, range]) {
                if rt.represented_type(input.as_ref())? != expected { return Err("diagnostic member witness mismatch".into()); }
            }
            let start = context.diagnostics().len();
            let callback: Callback = unsafe { std::mem::transmute(address as usize) };
            let status = unsafe { callback(context, inputs[1].words().as_ptr(), output.as_mut_ptr(), inputs[0].words().as_ptr()) };
            if status > 1 { return Err("invalid diagnostic callback status".into()); }
            let Some(reports) = context.take_scoped_diagnostics(start)? else { return Ok(Status::Failed); };
            let rt = context.runtime_mut()?;
            let reports = reports.iter().map(|report| rt.diagnostic_snapshot(diagnostic_ty, report)).collect::<Result<Vec<_>>>()?;
            let reports = rt.array(reports_ty, [0; 3], &reports)?;
            let value = if status == 0 {
                if never { return Err("Never callback returned successfully".into()); }
                let value = Value { arena: rt.identity, words: output.into() };
                rt.validate(value.as_ref(), returned)?;
                let tuple = rt.layout(ty)?.arguments[0];
                let payload = rt.aggregate(tuple, [0; 3], &[value, reports])?;
                rt.named_variant(ty, origin.words(), "Ok", Some(&payload))?
            } else { rt.named_variant(ty, origin.words(), "Err", Some(&reports))? };
            unsafe { std::ptr::copy_nonoverlapping(value.words().as_ptr(), out, value.words().len()); }
            Ok(Status::Success)
        })();
        match result { Ok(status) => status, Err(error) => context.abort_at(error, origin) }
    }) as u32
}

pub(super) unsafe fn format_parse(context: &mut CallContext, ty: TypeId, data: *const u64, out: *mut u64, origin: Origin, format: u64) -> u32 {
    context.boundary(|context| {
        let result = (|| -> Result<Status> {
            use telora_core::data_plan::{self, Format};
            let limits = context.data_limits();
            let rt = context.runtime()?;
            let witness_type = unsafe { TypeId((*data.add(1) >> 32) as u32) };
            let witness_width = rt.layout(witness_type)?.words;
            let witness = ValueRef { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data, witness_width) } };
            let target = rt.represented_type(witness)?;
            let input_data = unsafe { data.add(witness_width) };
            let input_type = unsafe { TypeId((*input_data.add(1) >> 32) as u32) };
            let input = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(input_data, rt.layout(input_type)?.words) }.into() };
            let contract = rt.data_contract.clone().ok_or("semantic Value contract is not loaded")?;
            if target != contract.value_type() { return Err("data parser target differs from sealed Value identity".into()); }
            let text = rt.text(input.as_ref())?;
            let size = text.as_str().len();
            if size > limits.file_size { return Ok(context.abort_at("parsed text exceeds file_size limit", origin)); }
            let (format, name) = match format { 0 => (Format::Json, "<json string>"), 1 => (Format::Yaml, "<yaml string>"), 2 => (Format::Toml, "<toml string>"), _ => return Err("unknown native data format".into()) };
            let mut sources = telora_core::source::SourceDatabase::default();
            let source = sources.add(name, text.as_str());
            let plan = data_plan::parse_registered(&sources, source, format);
            let loc = if input.origin().words()[0] == 0 { origin.words() } else { input.origin().words() };
            let value = match plan {
                Ok(plan) => {
                    if let Err(error) = data_plan::enforce_limits(&plan, limits, size) { return Ok(context.abort_at(error, origin)); }
                    let rt = context.runtime_mut()?;
                    let value = rt.materialize_data_at(&contract, &plan, Some(loc))?;
                    rt.named_variant(ty, origin.words(), "Ok", Some(&value))?
                }
                Err(diagnostics) => {
                    let message = diagnostics.iter().map(|diagnostic| sources.render(diagnostic)).collect::<Vec<_>>().join("\n");
                    let rt = context.runtime_mut()?;
                    let blame_type = rt.layout(ty)?.arguments[1];
                    let message = rt.owned_string(contract.payload("String")?, origin.words(), message)?;
                    let blame = rt.blame(blame_type, origin.words(), &message, vec![input.origin()])?;
                    rt.named_variant(ty, origin.words(), "Err", Some(&blame))?
                }
            };
            unsafe { std::ptr::copy_nonoverlapping(value.words().as_ptr(), out, value.words().len()); }
            Ok(Status::Success)
        })();
        match result { Ok(status) => status, Err(error) => context.fail_at(error, origin) }
    }) as u32
}

/// A codec packet borrows initializer and dispatcher addresses from its
/// generated adapter. No code address is stored in a runtime heap object.
pub(super) unsafe fn codec(context: &mut CallContext, ty: TypeId, data: *const u64, out: *mut u64, origin: Origin, count: u64, operation: u32) -> u32 {
    context.boundary(|context| {
        let result = (|| -> Result<Status> {
            let rt = context.runtime()?;
            let mut cursor = unsafe { *data } as *const u64;
            let mut inputs = Vec::with_capacity(3);
            for _ in 0..3 {
                let input = unsafe { TypeId((*cursor.add(1) >> 32) as u32) };
                let width = rt.layout(input)?.words;
                inputs.push(Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(cursor, width) }.into() });
                cursor = unsafe { cursor.add(width) };
            }
            let target = rt.represented_type(inputs[1].as_ref())?;
            if operation == helpers::JSON_SCHEMA && rt.represented_type(inputs[2].as_ref())? != ty { return Err("schema output witness mismatch".into()); }
            let check_count = (count as u32) as usize;
            let property_count = (count >> 32) as usize;
            let mut checks = Vec::with_capacity(check_count);
            for index in 0..check_count {
                let words = unsafe { std::slice::from_raw_parts(data.add(1 + index * 6), 6) };
                checks.push(codec::CheckPlan { owner: TypeId(words[0] as u32), site: words[1], slot: words[2], signature: TypeId(words[3] as u32), initializer: words[4] as usize, dispatcher: words[5] as usize });
            }
            let mut properties = Vec::with_capacity(property_count);
            for index in 0..property_count {
                let words = unsafe { std::slice::from_raw_parts(data.add(1 + check_count * 6 + index * 5), 5) };
                properties.push(codec::PropertyPlan { owner: TypeId(words[0] as u32), property: TypeId(words[1] as u32), slot: words[2], initializer: words[3] as usize, display_dispatcher: words[4] as usize });
            }
            let mut codec = codec::Codec { context, checks: &checks, properties: &properties };
            let value = if operation == helpers::PARSE { codec.parse(ty, target, &inputs[0], &inputs[2], origin.words())? }
                else if operation == helpers::JSON_SCHEMA { codec.schema(target, &inputs[0], origin.words())? }
                else if operation == helpers::DECODE { codec.decode(ty, target, &inputs[0], &inputs[2], origin.words())? }
                else {
                    if ty != target { return Err("codec target witness mismatch".into()); }
                    codec.encode(target, &inputs[0], &inputs[2])?
                };
            let Some(value) = value else { return Ok(Status::Failed); };
            let rt = context.runtime()?;
            rt.validate(value.as_ref(), ty)?;
            unsafe { std::ptr::copy_nonoverlapping(value.words().as_ptr(), out, value.words().len()); }
            Ok(Status::Success)
        })();
        match result { Ok(status) => status, Err(error) => context.fail_at(error, origin) }
    }) as u32
}

/// The packet contains a generated dispatcher, container, accumulator and
/// closure. Only descriptor words cross callbacks; heap objects stay in place.
pub(super) unsafe fn fold(context: &mut CallContext, ty: TypeId, data: *const u64, out: *mut u64, origin: Origin, operation: u64) -> u32 {
    context.boundary(|context| {
        let result = (|| {
            let rt = context.runtime()?;
            let dictionary = operation == 1;
            let controlled = operation == 2;
            let width = rt.layout(ty)?.words;
            let state_type = if controlled { rt.layout(ty)?.arguments[0] } else { ty };
            let state_width = rt.layout(state_type)?.words;
            let address = unsafe { *data };
            let container = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data.add(1), 4) }.into() };
            let mut accumulator = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data.add(5), state_width) }.into() };
            let closure = Value { arena: rt.identity, words: unsafe { std::slice::from_raw_parts(data.add(5 + state_width), 3) }.into() };
            rt.validate(accumulator.as_ref(), state_type)?;
            rt.function_id(&closure)?;
            let count = if dictionary { rt.dict_len(&container)? } else { rt.array_len(&container)? };
            let callback = unsafe { std::mem::transmute::<usize, Callback>(address as usize) };
            for index in 0..count {
                let rt = context.runtime()?;
                let mut arguments = accumulator.words().to_vec();
                if dictionary {
                    let (key, value) = rt.dict_entry(&container, index)?;
                    arguments.extend_from_slice(key.words());
                    arguments.extend_from_slice(value.words());
                } else { arguments.extend_from_slice(rt.array_get(&container, index)?.words()); }
                let mut words = vec![0; width].into_boxed_slice();
                let status = unsafe { callback(context, arguments.as_ptr(), words.as_mut_ptr(), closure.words().as_ptr()) };
                if status == 1 { return Ok(Status::Failed); }
                if status != 0 { return Err("native callback returned invalid status".into()); }
                let value = Value { arena: context.runtime()?.identity, words };
                let rt = context.runtime()?;
                rt.validate(value.as_ref(), ty)?;
                if controlled {
                    // The sealed native FoldControl ABI is Break=0, Continue=1.
                    if rt.enum_tag(&value)? == 0 {
                        unsafe { std::ptr::copy_nonoverlapping(value.words().as_ptr(), out, width); }
                        return Ok(Status::Success);
                    }
                    accumulator = rt.enum_payload(&value)?.ok_or("FoldControl lacks a state payload")?.to_owned();
                    rt.validate(accumulator.as_ref(), state_type)?;
                } else { accumulator = value; }
            }
            if controlled { accumulator = context.runtime_mut()?.named_variant(ty, origin.words(), "Continue", Some(&accumulator))?; }
            unsafe { std::ptr::copy_nonoverlapping(accumulator.words().as_ptr(), out, width); }
            Ok::<Status, String>(Status::Success)
        })();
        match result { Ok(status) => status, Err(error) => context.fail_at(error, origin) }
    }) as u32
}

/// All addresses and descriptor buffers are borrowed from the current native
/// call. Runtime borrows end before calling generated code recursively.
pub(super) unsafe fn array_map(
    context: &mut CallContext,
    ty: TypeId,
    data: *const u64,
    out: *mut u64,
    origin: Origin,
    operation: u64,
) -> u32 {
    context.boundary(|context| {
        let result = (|| {
            let rt = context.runtime()?;
            let dictionary = operation == 1 || operation == 6;
            let find = operation == 2;
            let filter = operation == 3 || operation == 6;
            let boolean = operation == 4 || operation == 5;
            let mut answer = operation == 5;
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
            let element = *rt.layout(closure.type_id())?.arguments.last().ok_or("missing callback result")?;
            let output_width = rt.layout(element)?.words;
            let count = if dictionary { rt.dict_len(&array)? } else { rt.array_len(&array)? };
            let callback = unsafe { std::mem::transmute::<usize, Callback>(address as usize) };
            let mut mapped = Vec::with_capacity(if boolean { 0 } else if find { 1 } else { count });
            let mut selected_keys = Vec::new();
            for index in 0..count {
                let rt = context.runtime()?;
                let argument = if dictionary { rt.dict_entry(&array, index)?.1 } else { rt.array_get(&array, index)? }.to_owned();
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
                if operation == 7 {
                    for index in 0..rt.array_len(&value)? { mapped.push(rt.array_get(&value, index)?.to_owned()); }
                    continue;
                }
                if find || filter || boolean {
                    let selected = rt.scalar_bits(value.as_ref())? != 0;
                    if boolean {
                        if selected != answer { answer = selected; break; }
                    } else if selected {
                        if dictionary { selected_keys.push(rt.dict_entry(&array, index)?.0.to_owned()); }
                        mapped.push(argument);
                        if find { break; }
                    }
                    continue;
                }
                mapped.push(value);
            }
            let rt = context.runtime_mut()?;
            let value = if boolean {
                rt.scalar(ty, origin.words(), u64::from(answer))?
            } else if find {
                rt.named_variant(ty, origin.words(), if mapped.is_empty() { "None" } else { "Some" }, mapped.first())?
            } else if dictionary && filter {
                let pairs = selected_keys.into_iter().zip(mapped).collect::<Vec<_>>();
                rt.dict(ty, origin.words(), &pairs)?
            } else if dictionary {
                // Keys are already canonical and immutable. Reuse their column;
                // only callback result descriptors need new storage.
                let words = mapped.iter().flat_map(|value| value.words().iter().copied()).collect();
                let values = rt.push_words(Table::Arrays, words)?;
                rt.pack(ty, origin.words(), &[array.words()[2], u64::from(values)])?
            } else {
                rt.array(ty, origin.words(), &mapped)?
            };
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
