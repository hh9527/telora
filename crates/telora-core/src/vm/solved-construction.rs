#[derive(Debug)]
struct SolvedCheckCall {
    node: crate::execution_graph::NodeId,
    argument: Val,
    return_target: ReturnTarget,
    trace_frame: RuntimeFrame,
    function: Arc<BytecodeFunction>,
    pc: usize,
}

impl NativeContinuation for SolvedCheckCall {
    fn return_target(&self) -> &ReturnTarget {
        &self.return_target
    }
    fn trace_frame(&self) -> &RuntimeFrame {
        &self.trace_frame
    }
    fn resume(
        self: Box<Self>,
        _: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        continue_solved_check(*self, current, background, account)
    }
    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        _: &mut Heap,
        _: &Heap,
        _: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        Ok(VmAction::Return {
            value: failure,
            return_target: self.return_target,
        })
    }
}

fn run_solved_construction_check(
    node: crate::execution_graph::NodeId,
    argument: Val,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    continue_solved_check(
        SolvedCheckCall {
            node,
            argument,
            return_target,
            trace_frame: RuntimeFrame {
                function: function.name().to_owned(),
                instruction: pc,
                origin: function.origin_at(pc),
            },
            function: Arc::new(function.clone()),
            pc,
        },
        current,
        background,
        account,
    )
}

fn continue_solved_check(
    state: SolvedCheckCall,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    use crate::execution_graph::{EvaluationError, Request};
    let function = Arc::clone(&state.function);
    let pc = state.pc;
    let rule_boundary = Some(
        background
            .solved_graph
            .as_ref()
            .expect("check graph")
            .nodes()[state.node.index()]
        .location,
    );
    consume_fuel(account, &function, pc)?;
    match request_solved(current, background, state.node)
    {
        Ok(Request::Ready(callee)) => Ok(VmAction::Call {
            callee: *callee,
            arguments: vec![state.argument],
            return_target: state.return_target,
            call_function: function,
            call_pc: pc,
            rule_boundary,
        }),
        Ok(Request::Start) => {
            let callee = current
                .solved_tasks
                .get(state.node.index())
                .copied()
                .flatten()
                .ok_or_else(|| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        "construction checker has no initializer",
                        &function,
                        pc,
                    )
                })?;
            let continuation = DemandContinuation {
                node: state.node,
                trace_frame: state.trace_frame.clone(),
                call_function: Arc::clone(&function),
                call_pc: pc,
                return_target: ReturnTarget::Native(Box::new(state)),
            };
            Ok(VmAction::Call {
                callee,
                arguments: vec![],
                return_target: ReturnTarget::Native(Box::new(continuation)),
                call_function: function,
                call_pc: pc,
                rule_boundary,
            })
        }
        Err(EvaluationError::Failed(failure)) => Err(propagated_failure_error(
            failure.0,
            state.argument.loc(),
            &function,
            pc,
        )),
        Err(EvaluationError::Cycle(path)) => Err(error(
            RuntimeErrorKind::UninitializedDefinition,
            format!("cyclic construction checker demand: {path:?}"),
            &function,
            pc,
        )),
        Err(e) => Err(error(
            RuntimeErrorKind::InvalidBytecode,
            format!("invalid construction checker demand: {e:?}"),
            &function,
            pc,
        )),
    }
}

fn solved_check_rejection(
    result: Val,
    current: &Heap,
    background: &Heap,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Option<Val>, RuntimeError> {
    propagate_direct_failure(&result, function, pc)?;
    let view = HeapView {
        current,
        background: Some(background),
    };
    let (tag, payload) = (ValueRef {
        value: result,
        view,
    })
    .tagged_parts()
    .ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "checker did not return Result",
            function,
            pc,
        )
    })?;
    match tag.as_atom().as_ref().map(|tag| tag.as_str()) {
        Some("Ok") => Ok(None),
        Some("Err") => Ok(Some(payload.value)),
        _ => Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "invalid checker result tag",
            function,
            pc,
        )),
    }
}

fn solved_construction_blame(
    blame: Val,
    rule_boundary: crate::Loc,
    current: &Heap,
    background: &Heap,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<RuntimeError, RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let invalid = || {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "construction rejection is not BlameError",
            function,
            pc,
        )
    };
    let DecodedValue::Opaque(handle) = blame.value() else {
        return Err(invalid());
    };
    let Object::Opaque(value) = view.object(handle).map_err(|_| invalid())? else {
        return Err(invalid());
    };
    let message = value
        .downcast_ref::<String>(&crate::core::blame_native_type())
        .ok_or_else(invalid)?;
    let mut failure = error(RuntimeErrorKind::RaisedBlame, message.clone(), function, pc);
    failure.set_contextual_locations(
        value.traced.iter().filter_map(|value| value.loc()),
        Some(rule_boundary),
        blame.loc(),
    );
    Ok(failure)
}
