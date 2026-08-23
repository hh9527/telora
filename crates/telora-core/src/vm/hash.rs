#[allow(clippy::too_many_arguments)]
fn run_core_hash(
    operation: CoreHashFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let input = view
        .string_text(arguments[0])
        .map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                function,
                pc,
            )
        })?
        .ok_or_else(|| {
            error(
                RuntimeErrorKind::TypeMismatch,
                format!("{} expects String", operation.name()),
                function,
                pc,
            )
        })?;
    let digest = match operation {
        CoreHashFunction::Sha256 => crate::sha256::hex(input.as_bytes()),
    };
    charge_allocation(account, digest.len() as u64, function, pc)?;
    Ok(VmAction::Return {
        value: Val::new(
            current.string(Some(background), &digest),
            instruction_location(function, pc),
        ),
        return_target,
    })
}

