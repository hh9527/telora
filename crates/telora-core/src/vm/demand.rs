// Initialization owns the mutable table. Once MainWorld is frozen, a read may
// only consume its completed result; it cannot restart initialization in Work.
fn request_solved<'a>(current: &'a mut Heap, main: &'a Heap, node: crate::execution_graph::NodeId)
    -> Result<crate::execution_graph::Request<'a, Val>, crate::execution_graph::EvaluationError>
{
    use crate::execution_graph::{EvaluationError, Request};
    if let Some(values) = &main.solved_evaluation {
        return values.ready(node).map(Request::Ready).ok_or(EvaluationError::InvalidNode(node));
    }
    current.solved_evaluation.as_mut().ok_or(EvaluationError::InvalidNode(node))?.request(node)
}

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
        // Publication fixes the initializer's existing source, rather than
        // treating the first read as a call that generated the exported value.
        // Cache hits and the first return must carry the identical handle,
        // solved type and provenance flags.
        let value = value.preserve_origin();
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
            return_target: match self.return_target {
                // A cache read is not a value-producing call. In particular,
                // an absent initializer origin must stay absent on first use.
                ReturnTarget::Register { destination, .. } => ReturnTarget::Register { destination, call_site: None },
                target => target,
            },
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

fn solved_some(
    value: Val,
    current: &mut Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    charge_allocation(
        account,
        logical_value_bytes(2).map_err(|e| allocation_error(e.message, function, pc))?,
        function,
        pc,
    )?;
    Ok(Val::new(
        DecodedValue::Tagged(current.allocate(Object::Tagged {
            tag: Val::unknown(DecodedValue::BuiltinAtom(crate::BuiltinAtom::Some)),
            payload: value,
        })),
        instruction_location(function, pc),
    ))
}
