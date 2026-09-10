#[derive(Debug)]
struct DemandContinuation {
    node: crate::execution_graph::NodeId,
    return_target: ReturnTarget,
    trace_frame: RuntimeFrame,
    call_function: Arc<BytecodeFunction>,
    call_pc: usize,
}

impl NativeContinuation for DemandContinuation {
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
        _: &Heap,
        _: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        current
            .solved_evaluation
            .as_mut()
            .expect("demand session")
            .complete(self.node, value)
            .map_err(|e| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    format!("invalid demand completion: {e:?}"),
                    &self.call_function,
                    self.call_pc,
                )
            })?;
        Ok(VmAction::Return {
            value,
            return_target: self.return_target,
        })
    }

    fn resume_failed(
        self: Box<Self>,
        failure: Val,
        current: &mut Heap,
        _: &Heap,
        _: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        let DecodedValue::Failed(id) = failure.value() else {
            unreachable!()
        };
        current
            .solved_evaluation
            .as_mut()
            .expect("demand session")
            .fail(self.node, crate::execution_graph::FailureId(id))
            .map_err(|e| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    format!("invalid demand failure: {e:?}"),
                    &self.call_function,
                    self.call_pc,
                )
            })?;
        Ok(VmAction::Return {
            value: failure,
            return_target: self.return_target,
        })
    }
}
