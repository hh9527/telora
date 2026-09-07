#[derive(Debug)]
struct CodecDecodeState {
    check_rejections_as_result: bool,
    tasks: Vec<DecodeTask>,
    values: Vec<Val>,
    rejection: Option<Val>,
    input: Val,
    return_target: ReturnTarget,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
}

#[derive(Debug)]
enum DecodeBuild {
    Array, Tuple, Dict(Vec<String>), Tagged, Semantic(Val),
}

#[derive(Debug)]
enum DecodeTask {
    Refine { descriptor: crate::types::TypeDescriptor, value: Val },
    Node(CodecNode),
    Build { kind: DecodeBuild, count: usize, loc: Option<crate::Loc> },
    Own { owner: Val, loc: Option<crate::Loc> },
    Trial(DecodeTrial),
}

#[derive(Debug)]
struct DecodeTrial {
    remaining: std::vec::IntoIter<CodecNode>,
    successes: Vec<Val>,
    errors: Vec<Val>,
    base: usize,
    input: Val,
    path: String,
    rejected: bool,
}

#[derive(Debug)]
struct CodecDecodeContinuation {
    state: CodecDecodeState,
    trace_frame: RuntimeFrame,
}

impl NativeContinuation for CodecDecodeContinuation {
    fn return_target(&self) -> &ReturnTarget { &self.state.return_target }
    fn trace_frame(&self) -> &RuntimeFrame { &self.trace_frame }
    fn resume(
        mut self: Box<Self>, value: Val, current: &mut Heap, background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        let view = HeapView { current, background: Some(background) };
        propagate_data_failures(&[value], &view, &self.state.call_function, self.state.call_pc)?;
        if !self.state.check_rejections_as_result {
            self.state.values.push(value);
            return drive_codec_decode(self.state, current, background, account);
        }
        let result = ValueRef { value, view };
        let (tag, payload) = result.tagged_parts().ok_or_else(|| runtime_type_error(
            "construction Result", &value, &view, &self.state.call_function, self.state.call_pc,
        ))?;
        if tag.as_atom().is_some_and(|tag| tag == "Ok") {
            self.state.values.push(payload.runtime());
        } else {
            self.state.rejection = Some(payload.runtime());
        }
        drive_codec_decode(self.state, current, background, account)
    }
    fn resume_failed(
        self: Box<Self>, failure: Val, _current: &mut Heap, _background: &Heap,
        _account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        Ok(VmAction::Return { value: failure, return_target: self.state.return_target })
    }
}

fn decode_blame(
    message: String, subjects: Vec<Val>, function: &BytecodeFunction, pc: usize,
    current: &mut Heap, account: &mut QuotaAccount,
) -> Result<Val, RuntimeError> {
    let bytes = logical_value_bytes(subjects.len().saturating_add(3))
        .and_then(|bytes| bytes.checked_add(message.len() as u64)
            .ok_or_else(|| NativeError::allocation_limit("decode error size overflowed")))
        .map_err(|err| allocation_error(err.message, function, pc))?;
    charge_allocation(account, bytes, function, pc)?;
    let location = subjects.first().and_then(|value| value.loc());
    let mut opaque = crate::OpaqueValue::new_identity(crate::core::blame_native_type(), message);
    opaque.traced = subjects.into_boxed_slice();
    Ok(Val::new(DecodedValue::Opaque(current.allocate(Object::Opaque(opaque))), location))
}

fn decode_leaf(
    node: CodecNode, function: &BytecodeFunction, pc: usize,
    current: &mut Heap, background: &Heap, account: &mut QuotaAccount,
) -> Result<Val, RuntimeError> {
    let bytes = codec_node_bytes(&node, current, background)
        .map_err(|err| match err.limit() {
            Some(_) => allocation_error(err.message, function, pc),
            None => error(RuntimeErrorKind::TypeMismatch, err.message, function, pc),
        })?;
    charge_allocation(account, bytes, function, pc)?;
    Ok(materialize_codec_node(node, current, background))
}

fn drive_codec_decode(
    mut state: CodecDecodeState, current: &mut Heap, background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let call_function = Arc::clone(&state.call_function);
    let function = call_function.as_ref();
    let pc = state.call_pc;
    loop {
        if let Some(rejection) = state.rejection.take() {
            let mut caught = false;
            while let Some(task) = state.tasks.pop() {
                if let DecodeTask::Trial(mut trial) = task {
                    state.values.truncate(trial.base);
                    trial.errors.push(rejection);
                    trial.rejected = true;
                    state.tasks.push(DecodeTask::Trial(trial));
                    caught = true;
                    break;
                }
            }
            if !caught {
                return finish_codec_payload(BuiltinAtom::Err, CodecNode::Existing(rejection), state.input,
                    state.return_target, function, pc, current, background, account);
            }
        }
        let Some(task) = state.tasks.pop() else {
            let value = state.values.pop().expect("decoded root");
            return finish_codec_payload(BuiltinAtom::Ok, CodecNode::Existing(value), state.input,
                state.return_target, function, pc, current, background, account);
        };
        consume_fuel(account, function, pc)?;
        match task {
            DecodeTask::Refine { descriptor, value } => {
                expand_cast_refinement(&mut state, descriptor, value, current, background, account)?;
            }
            DecodeTask::Node(CodecNode::Decode { schema, properties, value, path, input }) => {
                let node = transform_codec_inner(&schema, &properties, value, CodecDirection::Decode,
                    &path, current, background, input).unwrap_or_else(|mut failure| {
                        if failure.input.is_none() { failure.input = input; }
                        CodecNode::Reject(failure)
                    });
                state.tasks.push(DecodeTask::Node(node));
            }
            DecodeTask::Node(CodecNode::Reject(failure)) => {
                state.rejection = Some(decode_blame(failure.message,
                    vec![failure.input.unwrap_or(state.input)], function, pc, current, account)?);
            }
            DecodeTask::Node(CodecNode::Trials { mut variants, input, path }) => {
                if let Some(DecodeTask::Own { owner, loc }) = state.tasks.last() {
                    variants = variants.into_iter().map(|payload| CodecNode::Declared {
                        owner: *owner, payload: Box::new(payload), loc: *loc,
                    }).collect();
                }
                let mut remaining = variants.into_iter();
                let first = remaining.next();
                state.tasks.push(DecodeTask::Trial(DecodeTrial {
                    remaining, successes: Vec::new(), errors: Vec::new(), base: state.values.len(),
                    input, path, rejected: first.is_none(),
                }));
                if let Some(first) = first { state.tasks.push(DecodeTask::Node(first)); }
            }
            DecodeTask::Trial(mut trial) => {
                if !trial.rejected {
                    trial.successes.push(state.values.pop().expect("trial result"));
                }
                if let Some(next) = trial.remaining.next() {
                    trial.rejected = false;
                    state.tasks.push(DecodeTask::Trial(trial));
                    state.tasks.push(DecodeTask::Node(next));
                } else if trial.successes.len() == 1 {
                    state.values.push(trial.successes[0]);
                } else {
                    let view = HeapView { current, background: Some(background) };
                    let mut messages = Vec::new();
                    let mut subjects = vec![trial.input];
                    for (index, failure) in trial.errors.iter().enumerate() {
                        if let DecodedValue::Opaque(handle) = failure.value()
                            && let Ok(Object::Opaque(blame)) = view.object(handle)
                            && let Some(message) = blame.downcast_ref::<String>(&crate::core::blame_native_type())
                        {
                            messages.push(message.clone());
                            if index == 0 { subjects = blame.traced.to_vec(); }
                        }
                    }
                    let message = if trial.successes.is_empty() {
                        format!("{}: value matches no untagged Enum variant ({})", trial.path, messages.join("; "))
                    } else {
                        subjects = vec![trial.input];
                        format!("{}: value ambiguously matches multiple untagged Enum variants", trial.path)
                    };
                    state.rejection = Some(decode_blame(message, subjects, function, pc, current, account)?);
                }
            }
            DecodeTask::Node(CodecNode::Declared { owner, payload, loc }) => {
                state.tasks.push(DecodeTask::Own { owner, loc });
                state.tasks.push(DecodeTask::Node(*payload));
            }
            DecodeTask::Own { owner, loc } => {
                let value = state.values.pop().expect("declared payload").with_loc(loc);
                let target = HeapView { current, background: Some(background) }.declared_type_id(owner)
                    .map_err(|err| error(RuntimeErrorKind::TypeMismatch, err.to_string(), function, pc))?;
                let mut pending = Some(state);
                let return_result = pending.as_ref().unwrap().check_rejections_as_result;
                let action = construction_check_action(owner, value, target, || {
                    ReturnTarget::Native(Box::new(CodecDecodeContinuation {
                        state: pending.take().expect("decode state"),
                        trace_frame: RuntimeFrame { function: "codec.decode".into(), instruction: pc,
                            origin: function.origin_at(pc) },
                    }))
                }, return_result, Arc::clone(&call_function), pc, current, background, account)?;
                if let Some(action) = action { return Ok(action); }
                state = pending.expect("unchecked construction has no callback");
                state.values.push(value.with_type_id(target));
            }
            DecodeTask::Node(node) => {
                let (kind, nodes, loc) = match node {
                    CodecNode::Array(nodes, loc) => (DecodeBuild::Array, nodes, loc),
                    CodecNode::Tuple(nodes, loc) => (DecodeBuild::Tuple, nodes, loc),
                    CodecNode::Dict(fields, loc) => {
                        let (keys, nodes) = fields.into_iter().unzip();
                        (DecodeBuild::Dict(keys), nodes, loc)
                    }
                    CodecNode::Tagged { tag, payload, loc } => (DecodeBuild::Tagged, vec![*tag, *payload], loc),
                    CodecNode::SemanticValue { owner, raw } => (DecodeBuild::Semantic(owner), vec![*raw], None),
                    leaf => { state.values.push(decode_leaf(leaf, function, pc, current, background, account)?); continue; }
                };
                charge_allocation(account, logical_value_bytes(nodes.len().saturating_add(1))
                    .map_err(|err| allocation_error(err.message, function, pc))?, function, pc)?;
                state.tasks.push(DecodeTask::Build { kind, count: nodes.len(), loc });
                state.tasks.extend(nodes.into_iter().rev().map(DecodeTask::Node));
            }
            DecodeTask::Build { kind, count, loc } => {
                let nodes = state.values.split_off(state.values.len() - count).into_iter()
                    .map(CodecNode::Existing).collect::<Vec<_>>();
                let node = match kind {
                    DecodeBuild::Array => CodecNode::Array(nodes, loc),
                    DecodeBuild::Tuple => CodecNode::Tuple(nodes, loc),
                    DecodeBuild::Dict(keys) => CodecNode::Dict(keys.into_iter().zip(nodes).collect(), loc),
                    DecodeBuild::Tagged => { let mut nodes = nodes.into_iter(); CodecNode::Tagged {
                        tag: Box::new(nodes.next().unwrap()), payload: Box::new(nodes.next().unwrap()), loc,
                    } }
                    DecodeBuild::Semantic(owner) => CodecNode::SemanticValue { owner, raw: Box::new(nodes.into_iter().next().unwrap()) },
                };
                state.values.push(decode_leaf(node, function, pc, current, background, account)?);
            }
        }
    }
}
