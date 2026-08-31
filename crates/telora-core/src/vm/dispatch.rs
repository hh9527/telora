fn recoverable_instruction_destination(instruction: &Opcode) -> Option<Register> {
    match instruction {
        Opcode::LoadConst { dst, .. }
        | Opcode::Move { dst, .. }
        | Opcode::OwnDeclared { dst, .. }
        | Opcode::AllocFunc { dst, .. }
        | Opcode::AllocTypeSlot { dst }
        | Opcode::ReadTypeSlot { dst, .. }
        | Opcode::Add { dst, .. }
        | Opcode::Subtract { dst, .. }
        | Opcode::Multiply { dst, .. }
        | Opcode::Divide { dst, .. }
        | Opcode::Remainder { dst, .. }
        | Opcode::Negate { dst, .. }
        | Opcode::Not { dst, .. }
        | Opcode::LogicalNot { dst, .. }
        | Opcode::BitNot { dst, .. }
        | Opcode::BitAnd { dst, .. }
        | Opcode::BitOr { dst, .. }
        | Opcode::BitXor { dst, .. }
        | Opcode::Equal { dst, .. }
        | Opcode::NotEqual { dst, .. }
        | Opcode::LessThan { dst, .. }
        | Opcode::LessThanOrEqual { dst, .. }
        | Opcode::MakeArray { dst, .. }
        | Opcode::ConcatArrays { dst, .. }
        | Opcode::MakeTuple { dst, .. }
        | Opcode::InterpolateString { dst, .. }
        | Opcode::MakeDict { dst, .. }
        | Opcode::MergeDicts { dst, .. }
        | Opcode::GetField { dst, .. }
        | Opcode::GetArray { dst, .. }
        | Opcode::ProjectTuple { dst, .. }
        | Opcode::FieldExists { dst, .. }
        | Opcode::IsDict { dst, .. }
        | Opcode::TupleLengthEquals { dst, .. }
        | Opcode::GetTuple { dst, .. }
        | Opcode::TaggedTagEquals { dst, .. }
        | Opcode::GetTaggedPayload { dst, .. }
        | Opcode::MakeClosure { dst, .. } => Some(*dst),
        Opcode::Call { base, .. } => Some(*base),
        Opcode::Panic { message } => Some(*message),
        Opcode::Raise { error } => Some(*error),
        Opcode::SealFunc { .. }
        | Opcode::SealTypeSlot { .. }
        | Opcode::AssertTypeSlotReady { .. }
        | Opcode::TailCall { .. }
        | Opcode::Jump { .. }
        | Opcode::JumpIfFalse { .. }
        | Opcode::Return { .. }
        | Opcode::Fail { .. }
        | Opcode::Debug { .. } => None,
    }
}

fn append_runtime_trace(runtime_error: &mut RuntimeError, frames: &[ExecutionFrame]) {
    for (index, frame) in frames.iter().rev().enumerate() {
        if index != 0 || !runtime_error.trace_includes_active_frame {
            let instruction = frame.pc.saturating_sub(1);
            runtime_error.trace.push(RuntimeFrame {
                function: frame.function.name().to_owned(),
                instruction,
                origin: frame.function.origin_at(instruction),
            });
        }
        frame
            .return_target
            .append_native_trace(&mut runtime_error.trace);
    }
    runtime_error.trace_includes_active_frame = false;
}

fn make_execution_frame(
    function: Arc<BytecodeFunction>,
    prototype: Handle,
    arguments: &[Val],
    captures: &[Val],
    return_target: ReturnTarget,
    rule_boundary: Option<crate::Loc>,
    stack: &mut Vec<Option<Val>>,
    stack_limit: usize,
) -> Result<ExecutionFrame, RuntimeError> {
    if arguments.len() != function.parameter_count() {
        return Err(error(
            RuntimeErrorKind::TypeMismatch,
            format!(
                "expected {} arguments, got {}",
                function.parameter_count(),
                arguments.len()
            ),
            &function,
            0,
        ));
    }
    if captures.len() != function.capture_count() {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "closure capture count does not match function signature",
            &function,
            0,
        ));
    }
    let base = stack.len();
    let end = base.checked_add(function.register_count()).ok_or_else(|| {
        error(
            RuntimeErrorKind::StackLimitExceeded,
            "Telora stack size overflowed",
            &function,
            0,
        )
    })?;
    if end > stack_limit {
        return Err(error(
            RuntimeErrorKind::StackLimitExceeded,
            format!("Telora stack exceeds the limit of {stack_limit} slots"),
            &function,
            0,
        ));
    }
    stack.resize(end, None);
    for (index, value) in arguments.iter().chain(captures).enumerate() {
        let Some(register) = stack.get_mut(base + index) else {
            return Err(error(
                RuntimeErrorKind::InvalidBytecode,
                "function signature exceeds its register count",
                &function,
                0,
            ));
        };
        *register = Some(*value);
    }
    Ok(ExecutionFrame {
        function,
        prototype,
        base,
        pc: 0,
        return_target,
        rule_boundary,
    })
}

fn drive_vm_action(
    mut action: VmAction,
    frames: &mut Vec<ExecutionFrame>,
    stack: &mut Vec<Option<Val>>,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<DriveOutcome, RuntimeError> {
    loop {
        action = match action {
            VmAction::Return {
                value,
                return_target,
            } => match return_target {
                ReturnTarget::Root => return Ok(DriveOutcome::Root(value)),
                ReturnTarget::Register {
                    destination,
                    call_site,
                } => {
                    let caller = frames.last().ok_or_else(|| RuntimeError {
                        kind: RuntimeErrorKind::InvalidBytecode,
                        message: "return register has no caller".into(),
                        function: "<vm>".into(),
                        instruction: 0,
                        trace: Vec::new(),
                        locations: None,
                        rendered: None,
                        trace_includes_active_frame: false,
                        propagated_failure: None,
                    })?;
                    let caller_function = caller.function.clone();
                    let caller_end = caller.base + caller.function.register_count();
                    write_register(
                        &mut stack[caller.base..caller_end],
                        destination,
                        value.rebase_generated(call_site),
                        &caller_function,
                        caller.pc.saturating_sub(1),
                    )?;
                    return Ok(DriveOutcome::Pending);
                }
                ReturnTarget::Native(continuation) => {
                    let trace_frame = continuation.trace_frame().clone();
                    let resumed = if matches!(value.value(), DecodedValue::Failed(_)) {
                        continuation.resume_failed(value, current, background, account)
                    } else {
                        continuation.resume(value, current, background, account)
                    };
                    resumed.map_err(|mut runtime_error| {
                        runtime_error.trace.push(trace_frame);
                        runtime_error
                    })?
                }
            },
            VmAction::Call {
                callee,
                arguments,
                return_target,
                call_function,
                call_pc,
                rule_boundary,
            } => {
                consume_fuel(account, &call_function, call_pc).map_err(|mut runtime_error| {
                    return_target.append_native_trace(&mut runtime_error.trace);
                    runtime_error
                })?;
                let logical_depth = frames.len()
                    + frames
                        .iter()
                        .map(|frame| frame.return_target.native_depth())
                        .sum::<usize>()
                    + return_target.native_depth();
                if logical_depth >= MAX_CALL_DEPTH {
                    return Err(error(
                        RuntimeErrorKind::CallDepthExceeded,
                        format!("call depth exceeds the limit of {MAX_CALL_DEPTH} frames"),
                        &call_function,
                        call_pc,
                    ));
                }
                if matches!(
                    callee.value(),
                    DecodedValue::BuiltinAtom(_)
                        | DecodedValue::InlineAtom(_)
                        | DecodedValue::Atom(_)
                ) {
                    if arguments.len() != 1 {
                        return Err(error(
                            RuntimeErrorKind::TypeMismatch,
                            format!(
                                "tag constructor expects 1 argument, got {}",
                                arguments.len()
                            ),
                            &call_function,
                            call_pc,
                        ));
                    }
                    charge_allocation(
                        account,
                        (std::mem::size_of::<Val>() * 2) as u64,
                        &call_function,
                        call_pc,
                    )?;
                    let value = Val::new(
                        DecodedValue::Tagged(current.allocate(Object::Tagged {
                            tag: callee,
                            payload: arguments[0],
                        })),
                        callee.loc(),
                    );
                    VmAction::Return {
                        value,
                        return_target,
                    }
                } else {
                    let view = HeapView {
                        current,
                        background: Some(background),
                    };
                    let Some(closure_handle) = view.resolve_func(callee).map_err(|heap_error| {
                        error(
                            RuntimeErrorKind::UninitializedDefinition,
                            heap_error.to_string(),
                            &call_function,
                            call_pc,
                        )
                    })?
                    else {
                        return Err(runtime_type_error(
                            "Func",
                            &callee,
                            &view,
                            &call_function,
                            call_pc,
                        ));
                    };
                    if let Some(failure) = arguments.iter().find_map(|argument| {
                        if let DecodedValue::Failed(failure) = argument.value() {
                            Some((failure, argument.loc()))
                        } else {
                            None
                        }
                    }) {
                        return Err(propagated_failure_error(
                            failure.0,
                            failure.1,
                            &call_function,
                            call_pc,
                        ));
                    }
                    let (runtime_prototype, upvalues) =
                        view.closure(closure_handle).map_err(|heap_error| {
                            error(
                                RuntimeErrorKind::InvalidBytecode,
                                heap_error.to_string(),
                                &call_function,
                                call_pc,
                            )
                        })?;
                    let upvalues = upvalues.to_vec();
                    let expected_arity = match runtime_prototype {
                        crate::heap::RuntimePrototype::Bytecode(prototype) => view
                            .bytecode(prototype)
                            .map_err(|heap_error| {
                                error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    heap_error.to_string(),
                                    &call_function,
                                    call_pc,
                                )
                            })?
                            .0
                            .parameter_count(),
                        crate::heap::RuntimePrototype::Native(native) => native.arity(),
                    };
                    if arguments.len() != expected_arity {
                        return Err(error(
                            RuntimeErrorKind::TypeMismatch,
                            format!(
                                "expected {expected_arity} arguments, got {}",
                                arguments.len()
                            ),
                            &call_function,
                            call_pc,
                        ));
                    }
                    let memo = match runtime_prototype {
                        crate::heap::RuntimePrototype::Bytecode(prototype) => {
                            let (code, _, _, _) =
                                view.bytecode(prototype).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        &call_function,
                                        call_pc,
                                    )
                                })?;
                            if code.is_memoized_interpreter() {
                                let identity = view.function_identity(closure_handle).map_err(
                                    |heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            &call_function,
                                            call_pc,
                                        )
                                    },
                                )?;
                                let arguments = arguments
                                    .iter()
                                    .map(|argument| view.canonical_type_value_id(*argument))
                                    .collect::<Result<Vec<_>, _>>()
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::TypeMismatch,
                                            heap_error.to_string(),
                                            &call_function,
                                            call_pc,
                                        )
                                    })?;
                                let cached = current.memoized_interpreter(identity, &arguments);
                                Some((identity, arguments, cached))
                            } else {
                                None
                            }
                        }
                        crate::heap::RuntimePrototype::Native(_) => None,
                    };
                    if let Some((_, _, Some(value))) = memo {
                        VmAction::Return {
                            value,
                            return_target,
                        }
                    } else {
                        let return_target = if let Some((identity, arguments, None)) = memo {
                            ReturnTarget::Native(Box::new(InterpreterMemoContinuation {
                                identity,
                                arguments,
                                return_target,
                                trace_frame: RuntimeFrame {
                                    function: call_function.name().to_owned(),
                                    instruction: call_pc,
                                    origin: call_function.origin_at(call_pc),
                                },
                            }))
                        } else {
                            return_target
                        };
                        let inherited_rule_boundary = rule_boundary
                            .or_else(|| frames.last().and_then(|frame| frame.rule_boundary))
                            .or_else(|| instruction_location(&call_function, call_pc));
                    match runtime_prototype {
                        crate::heap::RuntimePrototype::Bytecode(prototype) => {
                            let (code, _, _, _) =
                                view.bytecode(prototype).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        &call_function,
                                        call_pc,
                                    )
                                })?;
                            let callee_function =
                                Arc::new(BytecodeFunction::from_linked_code(Arc::clone(code)));
                            let next = make_execution_frame(
                                callee_function,
                                prototype,
                                &arguments,
                                &upvalues,
                                return_target,
                                    inherited_rule_boundary,
                                stack,
                                account.stack_limit(),
                            )
                            .map_err(|runtime_error| {
                                error(
                                    runtime_error.kind,
                                    runtime_error.message,
                                    &call_function,
                                    call_pc,
                                )
                            })?;
                            frames.push(next);
                            return Ok(DriveOutcome::Pending);
                        }
                        crate::heap::RuntimePrototype::Native(native) => match native.kind() {
                            NativeKind::Synchronous => {
                                let mut context = CallContext::new(
                                    current,
                                    Some(background),
                                    stack,
                                    account,
                                    arguments,
                                    &upvalues,
                                    instruction_location(&call_function, call_pc),
                                )
                                .map_err(|native_error| {
                                    native_runtime_error(
                                        native,
                                        native_error,
                                        &call_function,
                                        call_pc,
                                    )
                                })?;
                                (native.callback())(&mut context).map_err(|native_error| {
                                    native_runtime_error(
                                        native,
                                        native_error,
                                        &call_function,
                                        call_pc,
                                    )
                                })?;
                                let value = context.take_result().map_err(|native_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        format!("{}: {}", native.name(), native_error.message),
                                        &call_function,
                                        call_pc,
                                    )
                                })?;
                                VmAction::Return {
                                    value: value.with_loc(
                                        value
                                            .loc()
                                            .or(instruction_location(&call_function, call_pc)),
                                    ),
                                    return_target,
                                }
                            }
                            NativeKind::CoreArray(function) => start_array_continuation(
                                function,
                                arguments,
                                return_target,
                                call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreModel(function) => run_core_model(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreBuiltinType(function) => run_core_builtin_type(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreDict(function) => {
                                if matches!(
                                    function,
                                    CoreDictFunction::MapValues
                                        | CoreDictFunction::Filter
                                        | CoreDictFunction::Fold
                                ) {
                                    start_dict_continuation(
                                        function,
                                        arguments,
                                        return_target,
                                        call_function,
                                        call_pc,
                                        current,
                                        background,
                                        account,
                                    )?
                                } else {
                                    run_core_dict(
                                        function,
                                        &arguments,
                                        return_target,
                                        &call_function,
                                        call_pc,
                                        current,
                                        background,
                                        account,
                                    )?
                                }
                            }
                            NativeKind::CoreString(function) => run_core_string(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CorePath(function) => run_core_path(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreDiagnostic(CoreDiagnosticFunction::Warn) => {
                                run_core_diagnostic(
                                    &arguments,
                                    return_target,
                                    &call_function,
                                    call_pc,
                                    current,
                                    background,
                                    account,
                                )?
                            }
                            NativeKind::CoreRuntime(operation) => run_core_runtime(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreHash(function) => run_core_hash(
                                function,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreCodec(operation) => run_core_codec(
                                operation,
                                &arguments,
                                return_target,
                                    inherited_rule_boundary,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreTypeDesc(operation) => run_core_type_desc(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreDyn(operation) => run_core_dyn(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                            NativeKind::CoreEq(operation) => run_core_eq(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                            )?,
                            NativeKind::CoreResult(operation) => run_core_result(
                                operation,
                                &arguments,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                            )?,
                            NativeKind::CoreJson(operation) => run_core_json(
                                operation,
                                &arguments,
                                &upvalues,
                                return_target,
                                &call_function,
                                call_pc,
                                current,
                                background,
                                account,
                            )?,
                        },
                    }
                }
            }
            }
        };
    }
}

impl ReturnTarget {
    fn native_depth(&self) -> usize {
        match self {
            Self::Root | Self::Register { .. } => 0,
            Self::Native(continuation) => 1 + continuation.return_target().native_depth(),
        }
    }

    fn append_native_trace(&self, trace: &mut Vec<RuntimeFrame>) {
        if let Self::Native(continuation) = self {
            trace.push(continuation.trace_frame().clone());
            continuation.return_target().append_native_trace(trace);
        }
    }
}

impl NativeContinuation for ArrayContinuation {
    fn return_target(&self) -> &ReturnTarget {
        &self.return_target
    }

    fn trace_frame(&self) -> &RuntimeFrame {
        &self.trace_frame
    }

    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_array_continuation(*self, value, current, background, account)
    }

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_array_failure(*self, failure, current, background, account)
    }
}

impl NativeContinuation for DictContinuation {
    fn return_target(&self) -> &ReturnTarget {
        &self.return_target
    }

    fn trace_frame(&self) -> &RuntimeFrame {
        &self.trace_frame
    }

    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_dict_continuation(*self, value, current, background, account)
    }

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_dict_failure(*self, failure, current, background, account)
    }
}

impl NativeContinuation for InterpreterMemoContinuation {
    fn return_target(&self) -> &ReturnTarget {
        &self.return_target
    }

    fn trace_frame(&self) -> &RuntimeFrame {
        &self.trace_frame
    }

    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        _background: &Heap,
        _account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        current.memoize_interpreter(self.identity, self.arguments, value);
        Ok(VmAction::Return {
            value,
            return_target: self.return_target,
        })
    }

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        _current: &mut Heap,
        _background: &Heap,
        _account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        Ok(VmAction::Return {
            value: failure,
            return_target: self.return_target,
        })
    }
}

impl NativeContinuation for CodecDisplayContinuation {
    fn return_target(&self) -> &ReturnTarget {
        &self.return_target
    }

    fn trace_frame(&self) -> &RuntimeFrame {
        &self.trace_frame
    }

    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        resume_codec_display(*self, value, current, background, account)
    }

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        _current: &mut Heap,
        _background: &Heap,
        _account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        Ok(VmAction::Return {
            value: failure,
            return_target: self.return_target,
        })
    }
}

fn native_runtime_error(
    native: crate::NativeFunction,
    native_error: NativeError,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    if native_error.is_non_finite_float() {
        let location = instruction_location(function, pc);
        let mut runtime = error(
            RuntimeErrorKind::RaisedBlame,
            "NonFiniteFloat",
            function,
            pc,
        );
        runtime.set_locations(location, location);
        return runtime;
    }
    error(
        match native_error.limit() {
            Some(NativeLimit::Stack) => RuntimeErrorKind::StackLimitExceeded,
            Some(NativeLimit::Allocation) => RuntimeErrorKind::AllocationQuotaExceeded,
            None => RuntimeErrorKind::TypeMismatch,
        },
        format!("{}: {}", native.name(), native_error.message),
        function,
        pc,
    )
}
