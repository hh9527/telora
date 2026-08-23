fn read_register<'a>(
    registers: &'a [Option<Val>],
    register: Register,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<&'a Val, RuntimeError> {
    registers
        .get(register.0)
        .ok_or_else(|| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                format!("register {} is out of bounds", register.0),
                function,
                pc,
            )
        })?
        .as_ref()
        .ok_or_else(|| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                format!("register {} is uninitialized", register.0),
                function,
                pc,
            )
        })
}

fn write_register(
    registers: &mut [Option<Val>],
    register: Register,
    value: Val,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    let slot = registers.get_mut(register.0).ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            format!("register {} is out of bounds", register.0),
            function,
            pc,
        )
    })?;
    *slot = Some(value);
    Ok(())
}

fn read_many(
    registers: &[Option<Val>],
    items: &[Register],
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Vec<Val>, RuntimeError> {
    items
        .iter()
        .map(|register| read_register(registers, *register, function, pc).copied())
        .collect()
}

fn read_call_arguments(
    registers: &[Option<Val>],
    base: Register,
    argument_count: usize,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Vec<Val>, RuntimeError> {
    let start = base.0.checked_add(1).ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "call window overflows",
            function,
            pc,
        )
    })?;
    let end = start.checked_add(argument_count).ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "call window overflows",
            function,
            pc,
        )
    })?;
    let arguments = registers.get(start..end).ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "call window is out of bounds",
            function,
            pc,
        )
    })?;
    arguments
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_ref().copied().ok_or_else(|| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    format!("call argument register {} is uninitialized", start + index),
                    function,
                    pc,
                )
            })
        })
        .collect()
}

#[derive(Clone, Copy)]
enum NumericOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Clone, Copy)]
enum BitwiseOperation {
    And,
    Or,
    Xor,
}

fn bitwise_binary(
    left: &Val,
    right: &Val,
    operation: BitwiseOperation,
    view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    let (DecodedValue::Int(left), DecodedValue::Int(right)) = (left.value(), right.value()) else {
        let invalid = if !matches!(left.value(), DecodedValue::Int(_)) {
            left
        } else {
            right
        };
        return Err(runtime_type_error("Int", invalid, view, function, pc));
    };
    let value = match operation {
        BitwiseOperation::And => left & right,
        BitwiseOperation::Or => left | right,
        BitwiseOperation::Xor => left ^ right,
    };
    Ok(DecodedValue::Int(value).into())
}

fn numeric_binary(
    left: &Val,
    right: &Val,
    operation: NumericOperation,
    view: &HeapView<'_>,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    match (left.value(), right.value()) {
        (DecodedValue::Int(left), DecodedValue::Int(right)) => {
            let value = match operation {
                NumericOperation::Add => left.checked_add(right),
                NumericOperation::Subtract => left.checked_sub(right),
                NumericOperation::Multiply => left.checked_mul(right),
                NumericOperation::Divide => left.checked_div(right),
                NumericOperation::Remainder => left.checked_rem(right),
            };
            let Some(value) = value else {
                let (kind, message) = match (operation, right) {
                    (NumericOperation::Divide, 0) => {
                        (RuntimeErrorKind::DivisionByZero, "integer division by zero")
                    }
                    (NumericOperation::Remainder, 0) => (
                        RuntimeErrorKind::DivisionByZero,
                        "integer remainder by zero",
                    ),
                    _ => (
                        RuntimeErrorKind::IntegerOverflow,
                        "integer arithmetic overflowed",
                    ),
                };
                return Err(error(kind, message, function, pc));
            };
            Ok(DecodedValue::Int(value).into())
        }
        (DecodedValue::Float(left_value), DecodedValue::Float(right_value)) => {
            let value = match operation {
                NumericOperation::Add => left_value + right_value,
                NumericOperation::Subtract => left_value - right_value,
                NumericOperation::Multiply => left_value * right_value,
                NumericOperation::Divide => left_value / right_value,
                NumericOperation::Remainder => left_value % right_value,
            };
            if !value.is_finite() {
                return Err(non_finite_float_error(account, left, right, function, pc));
            }
            Ok(DecodedValue::Float(value).into())
        }
        _ => Err(runtime_numeric_type_error(left, right, view, function, pc)),
    }
}

fn ordered_comparison(
    left: &Val,
    right: &Val,
    inclusive: bool,
    view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<bool, RuntimeError> {
    match (left.value(), right.value()) {
        (DecodedValue::Int(left), DecodedValue::Int(right)) => Ok(if inclusive {
            left <= right
        } else {
            left < right
        }),
        (DecodedValue::Float(left), DecodedValue::Float(right)) => Ok(if inclusive {
            left <= right
        } else {
            left < right
        }),
        (
            DecodedValue::InlineString(_) | DecodedValue::ShortString(_),
            DecodedValue::InlineString(_) | DecodedValue::ShortString(_),
        ) => {
            let left = view.string_text(*left).map_err(|heap_error| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            })?;
            let right = view.string_text(*right).map_err(|heap_error| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    heap_error.to_string(),
                    function,
                    pc,
                )
            })?;
            let (Some(left), Some(right)) = (left, right) else {
                unreachable!("String runtime values have text")
            };
            Ok(if inclusive {
                left.as_bytes() <= right.as_bytes()
            } else {
                left.as_bytes() < right.as_bytes()
            })
        }
        _ => Err(runtime_ordered_type_error(left, right, view, function, pc)),
    }
}

fn runtime_bool(value: bool) -> Val {
    DecodedValue::BuiltinAtom(if value {
        BuiltinAtom::True
    } else {
        BuiltinAtom::False
    })
    .into()
}

fn instruction_location(function: &BytecodeFunction, pc: usize) -> Option<crate::Loc> {
    match function.origin_at(pc) {
        Some(Origin::Source(location)) => Some(location),
        Some(Origin::Synthetic { derived_from }) => derived_from,
        None => None,
    }
}

fn runtime_type_error(
    expected: &str,
    actual: &Val,
    _view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    if let DecodedValue::Failed(failure) = actual.value() {
        return propagated_failure_error(failure, actual.loc(), function, pc);
    }
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        format!("expected {expected}, got {}", runtime_value_kind(*actual)),
        function,
        pc,
    );
    runtime_error.set_data_location(actual.loc());
    runtime_error
}

fn propagate_direct_failure(
    value: &Val,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    if let DecodedValue::Failed(failure) = value.value() {
        return Err(propagated_failure_error(failure, value.loc(), function, pc));
    }
    Ok(())
}

fn propagate_data_failures(
    values: &[Val],
    view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    for value in values {
        if let Some(failure) = view.first_data_failure(*value).map_err(|heap_error| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                heap_error.to_string(),
                function,
                pc,
            )
        })? {
            return Err(propagated_failure_error(failure, value.loc(), function, pc));
        }
    }
    Ok(())
}

fn runtime_shallow_type_error(
    expected: &str,
    actual: Val,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    let location = actual.loc();
    if let DecodedValue::Failed(failure) = actual.value() {
        return propagated_failure_error(failure, location, function, pc);
    }
    let actual_kind = runtime_value_kind(actual);
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        format!("expected {expected}, got {actual_kind}"),
        function,
        pc,
    );
    runtime_error.set_data_location(location);
    runtime_error
}

fn runtime_value_kind(actual: Val) -> &'static str {
    match actual.value() {
        DecodedValue::Failed(_) => unreachable!(),
        DecodedValue::Int(_) => "Int",
        DecodedValue::Float(_) => "Float",
        DecodedValue::BuiltinAtom(_) | DecodedValue::InlineAtom(_) | DecodedValue::Atom(_) => {
            "Atom"
        }
        DecodedValue::InlineString(_) | DecodedValue::ShortString(_) => "String",
        DecodedValue::Bytes(_) => "Bytes",
        DecodedValue::Opaque(_) => "Opaque",
        DecodedValue::NativeType(_) => "Type",
        DecodedValue::DeclaredType(_) | DecodedValue::SymbolicType(_) => "Type",
        DecodedValue::Array(_) => "Array",
        DecodedValue::Tuple(_) => "Tuple",
        DecodedValue::Tagged(_) => "Tagged",
        DecodedValue::Dict(_) => "Dict",
        DecodedValue::Func(_) => "Func",
        DecodedValue::FuncRef(_) => "Func",
        DecodedValue::Dyn(_) => "Dyn",
        DecodedValue::Module(_) => "Module",
        DecodedValue::TypeSlot(_) => "internal up-link",
    }
}

fn runtime_numeric_type_error(
    left: &Val,
    right: &Val,
    _view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    if let Some(failure) = [left, right]
        .into_iter()
        .find_map(|value| match value.value() {
            DecodedValue::Failed(failure) => Some(failure),
            _ => None,
        })
    {
        return propagated_failure_error(failure, left.loc().or(right.loc()), function, pc);
    }
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        format!(
            "numeric operands must have the same type, got {} and {}",
            runtime_value_kind(*left),
            runtime_value_kind(*right)
        ),
        function,
        pc,
    );
    runtime_error.set_data_location(left.loc().or(right.loc()));
    runtime_error
}

fn runtime_ordered_type_error(
    left: &Val,
    right: &Val,
    _view: &HeapView<'_>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    if let Some(failure) = [left, right]
        .into_iter()
        .find_map(|value| match value.value() {
            DecodedValue::Failed(failure) => Some(failure),
            _ => None,
        })
    {
        return propagated_failure_error(failure, left.loc().or(right.loc()), function, pc);
    }
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        format!(
            "ordered operands must be matching Int, Float, or String values, got {} and {}",
            runtime_value_kind(*left),
            runtime_value_kind(*right)
        ),
        function,
        pc,
    );
    runtime_error.set_data_location(left.loc().or(right.loc()));
    runtime_error
}

fn propagated_failure_error(
    failure: u32,
    location: Option<crate::Loc>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    let mut runtime_error = error(
        RuntimeErrorKind::TypeMismatch,
        "dependent computation received a failed evaluation node",
        function,
        pc,
    );
    runtime_error.propagated_failure = Some(failure);
    runtime_error.set_data_location(location);
    runtime_error
}

fn logical_value_bytes(count: usize) -> Result<u64, NativeError> {
    let count = u64::try_from(count)
        .map_err(|_| NativeError::allocation_limit("allocation item count overflowed"))?;
    let value_size = u64::try_from(std::mem::size_of::<Val>())
        .map_err(|_| NativeError::allocation_limit("Value size overflowed"))?;
    count
        .checked_mul(value_size)
        .ok_or_else(|| NativeError::allocation_limit("allocation size overflowed"))
}

fn allocation_error(
    message: impl Into<String>,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    error(
        RuntimeErrorKind::AllocationQuotaExceeded,
        message,
        function,
        pc,
    )
}

fn out_of_range_error(
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    // Equivalent to allocating the two-subject Tuple and three-field BlameError
    // produced by fail!("OutOfRange", receiver, index).
    let bytes = logical_value_bytes(5)
        .and_then(|bytes| {
            bytes
                .checked_add(15) // "data", "message", and "rule"
                .ok_or_else(|| NativeError::allocation_limit("allocation size overflowed"))
        })
        .map_err(|native_error| allocation_error(native_error.message, function, pc))
        .and_then(|bytes| charge_allocation(account, bytes, function, pc));
    if let Err(error) = bytes {
        return error;
    }
    let location = instruction_location(function, pc);
    let mut runtime = error(RuntimeErrorKind::RaisedBlame, "OutOfRange", function, pc);
    runtime.set_locations(location, location);
    runtime
}

fn non_finite_float_error(
    account: &mut QuotaAccount,
    left: &Val,
    right: &Val,
    function: &BytecodeFunction,
    pc: usize,
) -> RuntimeError {
    // Equivalent to allocating the two-subject Tuple and three-field BlameError
    // produced by fail!("NonFiniteFloat", left, right).
    let bytes = logical_value_bytes(5)
        .and_then(|bytes| {
            bytes
                .checked_add(15) // "data", "message", and "rule"
                .ok_or_else(|| NativeError::allocation_limit("allocation size overflowed"))
        })
        .map_err(|native_error| allocation_error(native_error.message, function, pc))
        .and_then(|bytes| charge_allocation(account, bytes, function, pc));
    if let Err(error) = bytes {
        return error;
    }
    let mut runtime = error(
        RuntimeErrorKind::RaisedBlame,
        "NonFiniteFloat",
        function,
        pc,
    );
    runtime.set_locations(
        left.loc().or(right.loc()),
        instruction_location(function, pc),
    );
    runtime
}

fn charge_allocation(
    account: &mut QuotaAccount,
    bytes: u64,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    account.charge_allocation(bytes).map_err(|()| {
        allocation_error(
            format!(
                "allocation quota of {} bytes exceeded",
                account.quota.allocation_bytes
            ),
            function,
            pc,
        )
    })
}

fn consume_fuel(
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    if let Some(query) = &account.query {
        query.check().map_err(|query_error| {
            error(
                RuntimeErrorKind::Cancelled,
                query_error.to_string(),
                function,
                pc,
            )
        })?;
    }
    if account.remaining_fuel == 0 {
        return Err(error(
            RuntimeErrorKind::FuelExhausted,
            "evaluation fuel exhausted",
            function,
            pc,
        ));
    }
    account.remaining_fuel -= 1;
    Ok(())
}

fn validate_jump(
    target: usize,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<(), RuntimeError> {
    if target >= function.instructions().len() {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            format!("jump target {target} is out of bounds"),
            function,
            pc,
        ));
    }
    Ok(())
}

fn error(
    kind: RuntimeErrorKind,
    message: impl Into<String>,
    function: &BytecodeFunction,
    instruction: usize,
) -> RuntimeError {
    RuntimeError {
        kind,
        message: message.into(),
        function: function.name().to_owned(),
        instruction,
        trace: vec![RuntimeFrame {
            function: function.name().to_owned(),
            instruction,
            origin: function.origin_at(instruction),
        }],
        locations: None,
        rendered: None,
        trace_includes_active_frame: true,
        propagated_failure: None,
    }
}

