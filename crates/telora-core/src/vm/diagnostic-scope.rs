fn run_core_runtime(
    operation: CoreRuntimeFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    call_function: &Arc<BytecodeFunction>,
    call_pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    match operation {
        CoreRuntimeFunction::CallWithDiagnostics => {
            let view = HeapView {
                current,
                background: Some(background),
            };
            let arity = view
                .resolved_function_arity(arguments[0])
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        call_function,
                        call_pc,
                    )
                })?
                .ok_or_else(|| {
                    runtime_type_error(
                        "Func",
                        &arguments[0],
                        &view,
                        call_function,
                        call_pc,
                    )
                })?;
            if arity != 1 {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    format!("rt.with_diagnostics callable must accept 1 argument, got {arity}"),
                    call_function,
                    call_pc,
                ));
            }
            let continuation = DiagnosticContinuation {
                diagnostic_start: account.diagnostics.len(),
                return_target,
                call_function: Arc::clone(call_function),
                call_pc,
                trace_frame: RuntimeFrame {
                    function: operation.name().into(),
                    instruction: 0,
                    origin: call_function.origin_at(call_pc),
                },
            };
            Ok(VmAction::Call {
                callee: arguments[0],
                arguments: vec![arguments[1]],
                return_target: ReturnTarget::Native(Box::new(continuation)),
                call_function: Arc::clone(call_function),
                call_pc,
                rule_boundary: None,
            })
        }
    }
}

impl NativeContinuation for DiagnosticContinuation {
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
        let reports = take_scoped_diagnostics(
            self.diagnostic_start,
            current,
            background,
            account,
            &self.call_function,
            self.call_pc,
        )?;
        diagnosed_result(
            Some(value),
            reports,
            self.return_target,
            current,
            account,
            &self.call_function,
            self.call_pc,
        )
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

    fn catches_recoverable(&self) -> bool {
        true
    }

    fn catch_recoverable(
        self: Box<Self>,
        error: RuntimeError,
        raised: Option<Val>,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        let mut reports = take_scoped_diagnostics(
            self.diagnostic_start,
            current,
            background,
            account,
            &self.call_function,
            self.call_pc,
        )?;
        reports.push(match raised {
            Some(value) => value,
            None => runtime_error_blame(
                &error,
                current,
                account,
                &self.call_function,
                self.call_pc,
            )?,
        });
        diagnosed_result(
            None,
            reports,
            self.return_target,
            current,
            account,
            &self.call_function,
            self.call_pc,
        )
    }
}

fn take_scoped_diagnostics(
    start: usize,
    current: &mut Heap,
    _background: &Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Vec<Val>, RuntimeError> {
    let diagnostics = account.diagnostics.drain(start..).collect::<Vec<_>>();
    diagnostics
        .iter()
        .map(|diagnostic| diagnostic_blame(diagnostic, current, account, function, pc))
        .collect()
}

fn diagnostic_blame(
    diagnostic: &Diagnostic,
    current: &mut Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    let primary = diagnostic
        .labels
        .iter()
        .find(|label| label.primary)
        .map(|label| label.location);
    let data = diagnostic
        .labels
        .iter()
        .filter(|label| !label.primary)
        .map(|label| Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)).with_loc(Some(label.location)))
        .collect::<Vec<_>>();
    make_blame(&diagnostic.message, primary, data, current, account, function, pc)
}

fn runtime_error_blame(
    runtime: &RuntimeError,
    current: &mut Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    let data = runtime
        .data_sources()
        .iter()
        .copied()
        .map(|location| Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)).with_loc(Some(location)))
        .collect::<Vec<_>>();
    make_blame(
        &runtime.message,
        runtime.rule_location().or_else(|| runtime.origin().and_then(|origin| match origin {
            Origin::Source(location) => Some(location),
            Origin::Synthetic { derived_from } => derived_from,
        })),
        data,
        current,
        account,
        function,
        pc,
    )
}

fn make_blame(
    message: &str,
    rule: Option<crate::Loc>,
    data: Vec<Val>,
    current: &mut Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    let bytes = logical_value_bytes(data.len().saturating_add(5))
        .and_then(|bytes| {
            bytes
                .checked_add(u64::try_from(message.len()).map_err(|_| {
                    NativeError::allocation_limit("diagnostic message size overflowed")
                })?)
                .ok_or_else(|| NativeError::allocation_limit("diagnostic size overflowed"))
        })
        .map_err(|error| allocation_error(error.message, function, pc))?;
    charge_allocation(account, bytes, function, pc)?;
    let data = Val::unknown(DecodedValue::Tuple(current.allocate(Object::Tuple(data.into()))));
    let message = Val::unknown(current.string(None, message));
    let rule = Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)).with_loc(rule);
    let names = ["data", "message", "rule"]
        .into_iter()
        .map(|name| current.intern(name))
        .collect();
    let shape = current.intern_shape(names);
    Ok(Val::unknown(DecodedValue::Dict(current.allocate(Object::Dict {
        shape,
        values: vec![data, message, rule].into_boxed_slice(),
    }))))
}

fn diagnosed_result(
    value: Option<Val>,
    reports: Vec<Val>,
    return_target: ReturnTarget,
    current: &mut Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<VmAction, RuntimeError> {
    let value_count = reports.len().saturating_add(if value.is_some() { 4 } else { 2 });
    let bytes = logical_value_bytes(value_count)
        .map_err(|error| allocation_error(error.message, function, pc))?;
    charge_allocation(account, bytes, function, pc)?;
    let reports = Val::unknown(DecodedValue::Array(current.allocate(Object::Array(reports.into()))));
    let (tag, payload) = match value {
        Some(value) => {
            let tuple = current.allocate(Object::Tuple(vec![value, reports].into()));
            (BuiltinAtom::Ok, Val::unknown(DecodedValue::Tuple(tuple)))
        }
        None => (BuiltinAtom::Err, reports),
    };
    let tagged = current.allocate(Object::Tagged {
        tag: Val::unknown(DecodedValue::BuiltinAtom(tag)),
        payload,
    });
    Ok(VmAction::Return {
        value: Val::unknown(DecodedValue::Tagged(tagged)),
        return_target,
    })
}
