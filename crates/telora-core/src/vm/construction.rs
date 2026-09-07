#[derive(Debug)]
struct ConstructionContinuation {
    return_result: bool,
    candidate: Val,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
    trace_frame: RuntimeFrame,
}

impl NativeContinuation for ConstructionContinuation {
    fn return_target(&self) -> &ReturnTarget { &self.return_target }
    fn trace_frame(&self) -> &RuntimeFrame { &self.trace_frame }

    fn resume(
        self: Box<Self>, value: Val, current: &mut Heap, background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        let view = HeapView { current, background: Some(background) };
        let function = &self.call_function;
        let pc = self.call_pc;
        propagate_data_failures(&[value], &view, function, pc)?;
        let result = ValueRef { value, view };
        if result.as_atom().is_some_and(|tag| tag == "None") {
            if self.return_result {
                return finish_codec_payload(BuiltinAtom::Ok, CodecNode::Existing(self.candidate),
                    self.candidate, self.return_target, function, pc, current, background, account);
            }
            return Ok(VmAction::Return { value: self.candidate, return_target: self.return_target });
        }
        if let Some((tag, blame)) = result.tagged_parts()
            && tag.as_atom().is_some_and(|tag| tag == "Some")
            && let DecodedValue::Opaque(handle) = blame.runtime().value()
            && let Ok(Object::Opaque(blame_value)) = view.object(handle)
            && let Some(message) = blame_value.downcast_ref::<String>(&crate::core::blame_native_type())
        {
            if self.return_result {
                return finish_codec_payload(BuiltinAtom::Err, CodecNode::Existing(blame.runtime()),
                    self.candidate, self.return_target, function, pc, current, background, account);
            }
            let mut failure = error(RuntimeErrorKind::RaisedBlame, message.clone(), function, pc);
            failure.set_contextual_locations(
                blame_value.traced.iter().filter_map(|value| value.loc()),
                instruction_location(function, pc), blame.runtime().loc(),
            );
            return Err(failure);
        }
        Err(runtime_type_error("Option(BlameError)", &value, &view, function, pc))
    }

    fn resume_failed(
        self: Box<Self>, failure: Val, _current: &mut Heap, _background: &Heap,
        _account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        Ok(VmAction::Return { value: failure, return_target: self.return_target })
    }
}

#[allow(clippy::too_many_arguments)]
fn construction_check_action(
    owner: Val, value: Val, target: crate::TypeId, return_target: impl FnOnce() -> ReturnTarget, return_result: bool,
    call_function: Arc<BytecodeFunction>, pc: usize,
    current: &mut Heap, background: &Heap, account: &mut QuotaAccount,
) -> Result<Option<VmAction>, RuntimeError> {
    if target.is_unchecked() || value.type_id() == Some(target) { return Ok(None); }
    let function = call_function.as_ref();
    let view = HeapView { current, background: Some(background) };
    let metadata = ValueRef { value: owner, view };
    let Some((id, _, _)) = metadata.declared_type_parts() else { return Ok(None); };
    let constructor = id.constructor();
    let input = ValueRef { value, view };
    let (key, argument) = if let Some((tag, payload)) = input.tagged_parts() {
        let tag = tag.as_atom().ok_or_else(|| runtime_type_error("enum tag", &value, &view, function, pc))?;
        let descriptor = crate::types::decode_type_ref(metadata, "construction target")
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        let crate::types::TypeDescriptor::Declared(declared) = descriptor else { return Ok(None); };
        let crate::types::TypeDescriptor::Enum(variants) = declared.body.as_ref() else { return Ok(None); };
        let index = variants.keys().position(|name| name == tag.as_str())
            .ok_or_else(|| error(RuntimeErrorKind::TypeMismatch, "unknown constructed variant", function, pc))?;
        (PropertyKey::Construction { constructor, variant: Some(index as u32) }, payload.runtime())
    } else {
        (PropertyKey::Construction { constructor, variant: None },
            input.sequence_get(0).map_or(value, |payload| payload.runtime()))
    };
    let Some(mut callback) = view.property(key) else { return Ok(None); };
    if matches!(callback.value(), DecodedValue::BuiltinAtom(BuiltinAtom::None)) {
        return Err(error(RuntimeErrorKind::TypeMismatch,
            "construction check is not initialized", function, pc));
    }
    let arguments = id.arguments().to_vec();
    let metadata_start = current.allocation_count();
    let mut argument = argument;
    if matches!(value.value(), DecodedValue::Dict(_)) {
        let descriptor = crate::types::decode_type_ref(metadata, "construction target")
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        let unchecked = crate::types::unchecked_descriptor(descriptor);
        current.type_descriptor_value(Some(background), &unchecked)
            .map_err(|err| error(RuntimeErrorKind::TypeMismatch, err.to_string(), function, pc))?;
        argument = value.with_type_id(target.unchecked());
    }
    if !arguments.is_empty() {
        let values = arguments.iter().map(|argument| current.type_descriptor_value(Some(background), argument))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| error(RuntimeErrorKind::TypeMismatch, err.to_string(), function, pc))?;
        let (instantiated, _) = crate::heap::instantiate_type_family(
            current, Some(background), callback, &values, &arguments,
        ).map_err(|err| error(RuntimeErrorKind::TypeMismatch, err.to_string(), function, pc))?;
        callback = instantiated;
    }
    charge_allocation(account, logical_value_bytes(current.allocation_count().saturating_sub(metadata_start).saturating_add(4))
        .map_err(|err| allocation_error(err.message, function, pc))?, function, pc)?;
    let continuation = ConstructionContinuation {
        return_result,
        candidate: value.with_type_id(target), return_target: return_target(),
        call_function: Arc::clone(&call_function), call_pc: pc,
        trace_frame: RuntimeFrame { function: "@check".into(), instruction: pc,
            origin: function.origin_at(pc) },
    };
    Ok(Some(VmAction::Call {
        callee: callback, arguments: vec![argument],
        return_target: ReturnTarget::Native(Box::new(continuation)),
        call_function, call_pc: pc, rule_boundary: None,
    }))
}
