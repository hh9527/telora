#[allow(clippy::too_many_arguments)]
fn start_checked_cast(
    owner: Val, result: Val, return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>, call_pc: usize,
    current: &mut Heap, background: &Heap, account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let view = HeapView { current, background: Some(background) };
    let reference = ValueRef { value: result, view };
    let Some((tag, payload)) = reference.tagged_parts() else {
        return Err(runtime_type_error("cast Result", &result, &view, &call_function, call_pc));
    };
    if !tag.as_atom().is_some_and(|tag| tag == "Ok") {
        return Ok(VmAction::Return { value: result, return_target });
    }
    let descriptor = crate::types::decode_type_ref(ValueRef { value: owner, view }, "cast target")
        .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, &call_function, call_pc))?;
    let input = payload.runtime();
    drive_codec_decode(CodecDecodeState {
        check_rejections_as_result: false,
        tasks: vec![DecodeTask::Refine { descriptor, value: input }],
        values: Vec::new(), rejection: None, input,
        return_target, call_function, call_pc,
    }, current, background, account)
}

fn expand_cast_refinement(
    state: &mut CodecDecodeState, descriptor: crate::types::TypeDescriptor, value: Val,
    current: &mut Heap, background: &Heap, account: &mut QuotaAccount,
) -> Result<(), RuntimeError> {
    use crate::types::TypeDescriptor as Ty;
    let view = HeapView { current, background: Some(background) };
    let input = ValueRef { value, view };
    if let Ty::Declared(declared) = &descriptor {
        if input.declared_value_parts().is_some_and(|(owner, _)| {
            owner.declared_type_parts().is_some_and(|(id, _, _)| id == &declared.id)
        }) {
            state.values.push(value);
            return Ok(());
        }
        let allocation_start = current.allocation_count();
        let owner = current.type_descriptor_value(Some(background), &descriptor)
            .map_err(|err| error(RuntimeErrorKind::TypeMismatch, err.to_string(),
                &state.call_function, state.call_pc))?;
        charge_allocation(account,
            logical_value_bytes(current.allocation_count().saturating_sub(allocation_start).saturating_add(2))
                .map_err(|err| allocation_error(err.message, &state.call_function, state.call_pc))?,
            &state.call_function, state.call_pc)?;
        state.tasks.push(DecodeTask::Own { owner, loc: value.loc() });
        state.tasks.push(DecodeTask::Refine { descriptor: (*declared.body).clone(), value });
        return Ok(());
    }
    let (kind, children): (DecodeBuild, Vec<(Ty, Val)>) = match &descriptor {
        Ty::Array(item) => (DecodeBuild::Array,
            (0..input.sequence_len().unwrap()).map(|index|
                ((**item).clone(), input.sequence_get(index).unwrap().runtime())).collect()),
        Ty::Tuple(items) => (DecodeBuild::Tuple,
            items.iter().enumerate().map(|(index, item)|
                (item.clone(), input.sequence_get(index).unwrap().runtime())).collect()),
        Ty::Newtype(item) => (DecodeBuild::Tuple,
            vec![((**item).clone(), input.sequence_get(0).unwrap().runtime())]),
        Ty::Struct(fields) => (DecodeBuild::Dict(fields.keys().cloned().collect()),
            fields.iter().map(|(name, item)|
                (item.clone(), input.dict_get(name).unwrap().runtime())).collect()),
        Ty::Dict(item) => {
            let names = input.dict_fields().unwrap();
            (DecodeBuild::Dict(names.iter().map(|name| name.to_string()).collect()),
                names.iter().map(|name|
                    ((**item).clone(), input.dict_get(name).unwrap().runtime())).collect())
        }
        Ty::Enum(variants) if input.tagged_parts().is_some() => {
            let (tag, payload) = input.tagged_parts().unwrap();
            let item = variants.get(tag.as_atom().unwrap().as_str()).unwrap().as_ref().unwrap();
            (DecodeBuild::Tagged, vec![(Ty::AtomValue, tag.runtime()), ((**item).clone(), payload.runtime())])
        }
        Ty::Tagged { payload: item, .. } => {
            let (tag, payload) = input.tagged_parts().unwrap();
            (DecodeBuild::Tagged, vec![(Ty::AtomValue, tag.runtime()), ((**item).clone(), payload.runtime())])
        }
        _ => { state.values.push(value); return Ok(()); }
    };
    charge_allocation(account, logical_value_bytes(children.len().saturating_add(1))
        .map_err(|err| allocation_error(err.message, &state.call_function, state.call_pc))?,
        &state.call_function, state.call_pc)?;
    state.tasks.push(DecodeTask::Build { kind, count: children.len(), loc: value.loc() });
    state.tasks.extend(children.into_iter().rev().map(|(descriptor, value)|
        DecodeTask::Refine { descriptor, value }));
    Ok(())
}
