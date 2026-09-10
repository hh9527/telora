fn solved_metadata_id(
    value: Val,
    types: &crate::type_image::TypeImage,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<crate::mir::TypeId, RuntimeError> {
    match value.value() {
        DecodedValue::SolvedType(id) if id.index() < types.types.len() => Ok(id),
        _ => Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "solved Dyn operation requires a TypeId from its session image",
            function,
            pc,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_solved_dyn(
    operation: CoreDynFunction,
    arguments: &[Val],
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let types = background
        .solved_types
        .as_ref()
        .expect("solved Dyn session");
    if operation == CoreDynFunction::Pack {
        solved_metadata_id(arguments[0], types, function, pc)?;
        // The generic signature proves the pairing before codegen. The VM stores
        // that witness and the original Val, without inspecting/copying its graph.
        let payload = arguments[1];
        propagate_direct_failure(&payload, function, pc)?;
        charge_allocation(
            account,
            logical_value_bytes(2).map_err(|e| allocation_error(e.message, function, pc))?,
            function,
            pc,
        )?;
        let handle = current.allocate(Object::Dyn {
            identity: Arc::new(()),
            descriptor: arguments[0],
            value: payload,
            scheme: None,
            origin: None,
        });
        return Ok(VmAction::Return {
            value: Val::new(DecodedValue::Dyn(handle), payload.loc()),
            return_target,
        });
    }
    let input = arguments[usize::from(operation == CoreDynFunction::ProjectWith)];
    propagate_direct_failure(&input, function, pc)?;
    let DecodedValue::Dyn(handle) = input.value() else {
        return Err(runtime_shallow_type_error("Dyn", input, function, pc));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let (_, descriptor, payload) = view
        .dyn_parts(handle)
        .map_err(|e| core_dict_heap_error(e, function, pc))?;
    let packaged = solved_metadata_id(descriptor, types, function, pc)?;
    let matches = match operation {
        CoreDynFunction::ProjectWith => {
            solved_metadata_id(arguments[0], types, function, pc)? == packaged
        }
        CoreDynFunction::Desc => {
            return Ok(VmAction::Return {
                value: descriptor,
                return_target,
            });
        }
        CoreDynFunction::CheckInt => {
            types.types[packaged.index()].constructor == crate::mir::TypeConstructor::Int
        }
        CoreDynFunction::CheckFloat => {
            types.types[packaged.index()].constructor == crate::mir::TypeConstructor::Float
        }
        CoreDynFunction::CheckString => {
            types.types[packaged.index()].constructor == crate::mir::TypeConstructor::String
        }
        CoreDynFunction::CheckBytes => {
            types.types[packaged.index()].constructor == crate::mir::TypeConstructor::Bytes
        }
        _ => {
            return Err(error(
                RuntimeErrorKind::InvalidBytecode,
                "this solved Dyn observation has no lowering yet",
                function,
                pc,
            ));
        }
    };
    let value = if matches {
        solved_some(payload, current, account, function, pc)?
    } else {
        Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), payload.loc())
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}
