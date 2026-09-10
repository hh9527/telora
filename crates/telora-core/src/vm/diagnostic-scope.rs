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
                    runtime_type_error("Func", &arguments[0], &view, call_function, call_pc)
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
                types: DiagnosticTypes {
                    diagnostic: arguments[2],
                    severity: arguments[3],
                    label: arguments[4],
                    range: arguments[5],
                },
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
            self.types,
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
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        let mut reports = take_scoped_diagnostics(
            self.diagnostic_start,
            self.types,
            current,
            background,
            account,
            &self.call_function,
            self.call_pc,
        )?;
        let diagnostic = error.diagnostic().unwrap_or_else(|| Diagnostic {
            severity: crate::source::Severity::Error,
            message: error.message.clone(),
            labels: Vec::new(),
            notes: Vec::new(),
        });
        reports.push(diagnostic_snapshot(
            &diagnostic,
            self.types,
            current,
            background,
            account,
            &self.call_function,
            self.call_pc,
        )?);
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
    types: DiagnosticTypes,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Vec<Val>, RuntimeError> {
    let diagnostics = account.diagnostics.drain(start..).collect::<Vec<_>>();
    diagnostics
        .iter()
        .map(|diagnostic| {
            diagnostic_snapshot(
                diagnostic, types, current, background, account, function, pc,
            )
        })
        .collect()
}

fn diagnostic_snapshot(
    diagnostic: &Diagnostic,
    types: DiagnosticTypes,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    if let Some(image) = &background.solved_types {
        for owner in [types.diagnostic, types.severity, types.label, types.range] {
            solved_metadata_id(owner, image, function, pc)?;
        }
    }
    let declared = |owner, payload| CodecNode::Declared {
        owner,
        payload: Box::new(payload),
        loc: None,
    };
    let labels = diagnostic
        .labels
        .iter()
        .map(|label| {
            let location = label.location;
            let source = account
                .source_names
                .get(&location.source)
                .map(|name| name.to_string())
                .unwrap_or_else(|| format!("source:{}", location.source.get()));
            let range = declared(
                types.range,
                CodecNode::Dict(
                    vec![
                        ("source".into(), CodecNode::String(source, None)),
                        (
                            "start".into(),
                            CodecNode::Existing(Val::unknown(DecodedValue::Int(i64::from(
                                location.start,
                            )))),
                        ),
                        (
                            "end".into(),
                            CodecNode::Existing(Val::unknown(DecodedValue::Int(i64::from(
                                location.end,
                            )))),
                        ),
                    ],
                    None,
                ),
            );
            declared(
                types.label,
                CodecNode::Dict(
                    vec![
                        ("location".into(), range),
                        (
                            "message".into(),
                            CodecNode::String(label.message.clone(), None),
                        ),
                        (
                            "primary".into(),
                            CodecNode::Atom(
                                if label.primary {
                                    BuiltinAtom::True
                                } else {
                                    BuiltinAtom::False
                                },
                                None,
                            ),
                        ),
                    ],
                    None,
                ),
            )
        })
        .collect();
    let severity = match diagnostic.severity {
        crate::source::Severity::Error => "Error",
        crate::source::Severity::Warning => "Warning",
        crate::source::Severity::Info => "Info",
    };
    let node = declared(
        types.diagnostic,
        CodecNode::Dict(
            vec![
                (
                    "severity".into(),
                    declared(types.severity, CodecNode::NamedAtom(severity.into(), None)),
                ),
                (
                    "message".into(),
                    CodecNode::String(diagnostic.message.clone(), None),
                ),
                ("labels".into(), CodecNode::Array(labels, None)),
                (
                    "notes".into(),
                    CodecNode::Array(
                        diagnostic
                            .notes
                            .iter()
                            .map(|note| CodecNode::String(note.clone(), None))
                            .collect(),
                        None,
                    ),
                ),
            ],
            None,
        ),
    );
    let bytes = codec_node_bytes(&node, current, background)
        .map_err(|error| allocation_error(error.message, function, pc))?;
    charge_allocation(account, bytes, function, pc)?;
    Ok(materialize_codec_node(node, current, background))
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
    let value_count = reports
        .len()
        .saturating_add(if value.is_some() { 4 } else { 2 });
    let bytes = logical_value_bytes(value_count)
        .map_err(|error| allocation_error(error.message, function, pc))?;
    charge_allocation(account, bytes, function, pc)?;
    let reports = Val::unknown(DecodedValue::Array(
        current.allocate(Object::Array(reports.into())),
    ));
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
