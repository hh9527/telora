#[derive(Debug)]
enum SolvedEncodeTask {
    Dict {
        shape: crate::heap::ShapeId,
        count: usize,
        loc: Option<crate::Loc>,
    },
    Visit {
        value: Val,
        ty: crate::mir::TypeId,
    },
    Array {
        count: usize,
        loc: Option<crate::Loc>,
    },
    Object {
        names: Vec<String>,
        loc: Option<crate::Loc>,
    },
}

#[derive(Debug)]
struct SolvedEncode {
    pending: Vec<SolvedEncodeTask>,
    output: Vec<Val>,
    target: crate::mir::TypeId,
    properties: Vec<(&'static str, crate::mir::TypeId)>,
    return_target: ReturnTarget,
    trace_frame: RuntimeFrame,
    function: Arc<BytecodeFunction>,
    pc: usize,
    rule_boundary: Option<crate::Loc>,
}

impl NativeContinuation for SolvedEncode {
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
        continue_solved_encode(*self, current, background, account)
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

#[derive(Debug)]
struct SolvedDisplay {
    encoder: SolvedEncode,
    loc: Option<crate::Loc>,
}

impl NativeContinuation for SolvedDisplay {
    fn return_target(&self) -> &ReturnTarget {
        &self.encoder.return_target
    }
    fn trace_frame(&self) -> &RuntimeFrame {
        &self.encoder.trace_frame
    }
    fn resume(
        self: Box<Self>,
        value: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        let Self { mut encoder, loc } = *self;
        let function = &encoder.function;
        let pc = encoder.pc;
        propagate_direct_failure(&value, function, pc)?;
        let reference = ValueRef {
            value,
            view: HeapView {
                current,
                background: Some(background),
            },
        };
        let render_error = |e: NativeError| {
            error(
                match e.limit() {
                    Some(NativeLimit::Stack) => RuntimeErrorKind::StackLimitExceeded,
                    Some(NativeLimit::Allocation) => RuntimeErrorKind::AllocationQuotaExceeded,
                    None => RuntimeErrorKind::TypeMismatch,
                },
                e.message,
                function,
                pc,
            )
        };
        let length = crate::fmt::rendered_value_len(reference).map_err(render_error)?;
        charge_allocation(account, length as u64, function, pc)?;
        let text = crate::fmt::render_value(reference).map_err(render_error)?;
        let text = Val::new(current.string(Some(background), &text), loc);
        encoder.output.push(solved_codec_tag(
            "String",
            text,
            encoder.target,
            loc,
            current,
            background,
            account,
            function,
            pc,
        )?);
        continue_solved_encode(encoder, current, background, account)
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
            return_target: self.encoder.return_target,
        })
    }
}

fn run_solved_codec_encode(
    arguments: &[Val],
    signature: Option<Val>,
    return_target: ReturnTarget,
    rule_boundary: Option<crate::Loc>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let types = background.solved_types.as_ref().expect("solved codec");
    let signature = signature.ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "codec requires a compiled native signature",
            function,
            pc,
        )
    })?;
    let signature = solved_metadata_id(signature, types, function, pc)?;
    let signature = &types.types[signature.index()];
    let target = solved_metadata_id(arguments[1], types, function, pc)?;
    if signature.constructor != crate::mir::TypeConstructor::Function
        || signature.arguments.len() != 4
        || signature.arguments[3] != target
    {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "codec encode signature does not match its target",
            function,
            pc,
        ));
    }
    let view = HeapView {
        current,
        background: Some(background),
    };
    let properties = ValueRef {
        value: arguments[0],
        view,
    };
    let properties = [
        "decode_by_parse",
        "encode_by_display",
        "json_rename_all",
        "json_untagged",
        "display_by",
    ]
    .into_iter()
    .map(|name| {
        let value = properties.dict_get(name).ok_or_else(|| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                "codec property contract is missing a field",
                function,
                pc,
            )
        })?;
        Ok((name, solved_metadata_id(value.value, types, function, pc)?))
    })
    .collect::<Result<Vec<_>, RuntimeError>>()?;
    continue_solved_encode(
        SolvedEncode {
            pending: vec![SolvedEncodeTask::Visit {
                value: arguments[2],
                ty: signature.arguments[2],
            }],
            output: vec![],
            target,
            rule_boundary,
            properties,
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

fn solved_codec_tag(
    tag: &str,
    mut payload: Val,
    target: crate::mir::TypeId,
    loc: Option<crate::Loc>,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    if tag == "Object" {
        // The native encoder constructs this dictionary, so it must carry the
        // same closed Dict(Value) witness as a source-level Value.Object call.
        // Read the target's applied layout; never synthesize a runtime type.
        let types = background.solved_types.as_ref().expect("solved codec image");
        let payload_type = (|| {
            let crate::mir::TypeConstructor::Nominal(symbol) = types.types[target.index()].constructor else { return None; };
            let index = types.definition(symbol)?.members.iter().position(|member| member.name == tag)?;
            let payload_type = types.layout(target)?.members[index]?;
            let shape = &types.types[payload_type.index()];
            (shape.constructor == crate::mir::TypeConstructor::Dict && shape.arguments == [target]).then_some(payload_type)
        })().ok_or_else(|| error(RuntimeErrorKind::InvalidBytecode,
            "codec target has no closed Dict(Value) object payload", function, pc))?;
        payload = payload.with_type_id(crate::TypeId::solved(payload_type));
    }
    charge_allocation(
        account,
        logical_value_bytes(2).map_err(|e| allocation_error(e.message, function, pc))?,
        function,
        pc,
    )?;
    let tag = Val::new(current.atom(Some(background), tag), loc);
    Ok(Val::new(
        DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
        loc,
    )
    .with_type_id(crate::TypeId::solved(target)))
}

fn continue_solved_encode(
    mut state: SolvedEncode,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    use crate::execution_graph::{EvaluationError, Request};
    use crate::mir::{TypeConstructor as T, TypeOperation};
    let types = background
        .solved_types
        .as_ref()
        .expect("solved codec image");
    let graph = background
        .solved_graph
        .as_ref()
        .expect("solved codec graph");
    while let Some(task) = state.pending.pop() {
        let function = Arc::clone(&state.function);
        let pc = state.pc;
        consume_fuel(account, &function, pc)?;
        let (value, ty) = match task {
            SolvedEncodeTask::Dict { shape, count, loc } => {
                let values = state
                    .output
                    .split_off(state.output.len() - count)
                    .into_boxed_slice();
                charge_allocation(
                    account,
                    logical_value_bytes(count)
                        .map_err(|e| allocation_error(e.message, &function, pc))?,
                    &function,
                    pc,
                )?;
                let payload = Val::new(
                    DecodedValue::Dict(current.allocate(Object::Dict { shape, values })),
                    loc,
                );
                state.output.push(solved_codec_tag(
                    "Object",
                    payload,
                    state.target,
                    loc,
                    current,
                    background,
                    account,
                    &function,
                    pc,
                )?);
                continue;
            }
            SolvedEncodeTask::Array { count, loc } => {
                let values = state.output.split_off(state.output.len() - count);
                charge_allocation(
                    account,
                    logical_value_bytes(count)
                        .map_err(|e| allocation_error(e.message, &function, pc))?,
                    &function,
                    pc,
                )?;
                let payload = Val::new(
                    DecodedValue::Array(current.allocate(Object::Array(values.into_boxed_slice()))),
                    loc,
                );
                state.output.push(solved_codec_tag(
                    "Array",
                    payload,
                    state.target,
                    loc,
                    current,
                    background,
                    account,
                    &function,
                    pc,
                )?);
                continue;
            }
            SolvedEncodeTask::Object { names, loc } => {
                let values = state.output.split_off(state.output.len() - names.len());
                charge_allocation(
                    account,
                    logical_value_bytes(names.len())
                        .map_err(|e| allocation_error(e.message, &function, pc))?,
                    &function,
                    pc,
                )?;
                let payload = current
                    .record_value(names.into_iter().zip(values))
                    .map_err(|e| {
                        error(
                            RuntimeErrorKind::InvalidBytecode,
                            e.to_string(),
                            &function,
                            pc,
                        )
                    })?;
                state.output.push(solved_codec_tag(
                    "Object",
                    payload,
                    state.target,
                    loc,
                    current,
                    background,
                    account,
                    &function,
                    pc,
                )?);
                continue;
            }
            SolvedEncodeTask::Visit { value, ty } => (value, ty),
        };
        propagate_direct_failure(&value, &function, pc)?;
        let mut rename = false;
        let mut untagged = false;
        let shape = &types.types[ty.index()];
        if matches!(shape.constructor, T::Nominal(_)) && ty != state.target {
            let has_property = |name| {
                state
                    .properties
                    .iter()
                    .find(|(key, _)| *key == name)
                    .and_then(|(_, property)| {
                        graph.property(crate::execution_graph::PropertyKey {
                            owner: ty,
                            site: crate::mir::PropertySite::Type,
                            property: *property,
                        })
                    })
                    .is_some()
            };
            let bridged = has_property("encode_by_display");
            if bridged != has_property("decode_by_parse") {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/string.decode_by_parse and std/string.encode_by_display must be used together",
                    &function,
                    pc,
                ));
            }
            let mut display = None;
            for &(name, property) in &state.properties {
                if bridged && matches!(name, "json_untagged" | "json_rename_all")
                    || !bridged && name == "display_by"
                {
                    continue;
                }
                let Some(node) = graph.property(crate::execution_graph::PropertyKey {
                    owner: ty,
                    site: crate::mir::PropertySite::Type,
                    property,
                }) else {
                    continue;
                };
                let result = current
                    .solved_evaluation
                    .as_mut()
                    .expect("solved codec state")
                    .request(node);
                let property = match result {
                    Ok(Request::Ready(value)) => *value,
                    Ok(Request::Start) => {
                        let callee = current
                            .solved_tasks
                            .get(node.index())
                            .copied()
                            .flatten()
                            .ok_or_else(|| {
                                error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "codec property has no compiled initializer",
                                    &function,
                                    pc,
                                )
                            })?;
                        state.pending.push(SolvedEncodeTask::Visit { value, ty });
                        let rule_boundary = state.rule_boundary;
                        let continuation = DemandContinuation {
                            node,
                            trace_frame: state.trace_frame.clone(),
                            call_function: Arc::clone(&function),
                            call_pc: pc,
                            return_target: ReturnTarget::Native(Box::new(state)),
                        };
                        return Ok(VmAction::Call {
                            callee,
                            arguments: vec![],
                            return_target: ReturnTarget::Native(Box::new(continuation)),
                            call_function: function,
                            call_pc: pc,
                            rule_boundary,
                        });
                    }
                    Err(EvaluationError::Failed(failure)) => {
                        return Err(propagated_failure_error(
                            failure.0,
                            value.loc(),
                            &function,
                            pc,
                        ));
                    }
                    Err(EvaluationError::Cycle(path)) => {
                        return Err(error(
                            RuntimeErrorKind::UninitializedDefinition,
                            format!("cyclic codec property demand: {path:?}"),
                            &function,
                            pc,
                        ));
                    }
                    Err(e) => {
                        return Err(error(
                            RuntimeErrorKind::InvalidBytecode,
                            format!("invalid codec property demand: {e:?}"),
                            &function,
                            pc,
                        ));
                    }
                };
                match name {
                    "decode_by_parse" | "encode_by_display" => {}
                    "display_by" => {
                        let view = HeapView {
                            current,
                            background: Some(background),
                        };
                        display = Some(
                            (ValueRef {
                                value: property,
                                view,
                            })
                            .dict_get("display")
                            .ok_or_else(|| {
                                error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "DisplayBy has no display function",
                                    &function,
                                    pc,
                                )
                            })?
                            .value,
                        );
                    }
                    "json_untagged" => untagged = true,
                    "json_rename_all" => {
                        let view = HeapView {
                            current,
                            background: Some(background),
                        };
                        let case = (ValueRef {
                            value: property,
                            view,
                        })
                        .dict_get("case")
                        .and_then(|v| v.as_atom());
                        if case
                            .as_ref()
                            .is_none_or(|case| case.as_str() != "CamelCase")
                        {
                            return Err(error(
                                RuntimeErrorKind::TypeMismatch,
                                "rename_all requires CamelCase",
                                &function,
                                pc,
                            ));
                        }
                        rename = true;
                    }
                    _ => unreachable!("codec property contract"),
                }
            }
            if bridged {
                let callee = display.ok_or_else(|| {
                    error(
                        RuntimeErrorKind::TypeMismatch,
                        "text codec requires a DisplayBy property",
                        &function,
                        pc,
                    )
                })?;
                let argument = pack_solved_dyn(ty, value, current, account, &function, pc)?;
                let rule_boundary = state.rule_boundary;
                return Ok(VmAction::Call {
                    callee,
                    arguments: vec![argument],
                    return_target: ReturnTarget::Native(Box::new(SolvedDisplay {
                        encoder: state,
                        loc: value.loc(),
                    })),
                    call_function: function,
                    call_pc: pc,
                    rule_boundary,
                });
            }
        }
        if ty == state.target {
            state.output.push(value);
            continue;
        }
        let loc = value.loc().or_else(|| instruction_location(&function, pc));
        let view = HeapView {
            current,
            background: Some(background),
        };
        let reference = ValueRef { value, view };
        match &shape.constructor {
            T::Dict => {
                let DecodedValue::Dict(handle) = value.value() else {
                    return Err(error(
                        RuntimeErrorKind::InvalidBytecode,
                        "dictionary does not match its solved type",
                        &function,
                        pc,
                    ));
                };
                let Object::Dict {
                    shape: dict_shape,
                    values,
                } = view.object(handle).map_err(|e| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        e.to_string(),
                        &function,
                        pc,
                    )
                })?
                else {
                    unreachable!()
                };
                state.pending.push(SolvedEncodeTask::Dict {
                    shape: *dict_shape,
                    count: values.len(),
                    loc,
                });
                for &value in values.iter().rev() {
                    state.pending.push(SolvedEncodeTask::Visit {
                        value,
                        ty: shape.arguments[0],
                    });
                }
            }
            T::Int | T::Float | T::String | T::Bytes => {
                let tag = match shape.constructor {
                    T::Int => "Int",
                    T::Float => "Float",
                    T::String => "String",
                    _ => "Bytes",
                };
                state.output.push(solved_codec_tag(
                    tag,
                    value,
                    state.target,
                    loc,
                    current,
                    background,
                    account,
                    &function,
                    pc,
                )?);
            }
            T::Bool => state
                .output
                .push(value.with_type_id(crate::TypeId::solved(state.target))),
            T::Array | T::Tuple => {
                let count = reference.sequence_len().ok_or_else(|| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        "codec sequence does not match its solved type",
                        &function,
                        pc,
                    )
                })?;
                if shape.constructor == T::Tuple && count != shape.arguments.len() {
                    return Err(error(
                        RuntimeErrorKind::InvalidBytecode,
                        "codec tuple arity does not match its solved type",
                        &function,
                        pc,
                    ));
                }
                state.pending.push(SolvedEncodeTask::Array { count, loc });
                for index in (0..count).rev() {
                    let value = reference.sequence_get(index).unwrap().value;
                    let ty = if shape.constructor == T::Array {
                        shape.arguments[0]
                    } else {
                        shape.arguments[index]
                    };
                    state.pending.push(SolvedEncodeTask::Visit { value, ty });
                }
            }
            T::Record(names) => {
                state.pending.push(SolvedEncodeTask::Object {
                    names: names.clone(),
                    loc,
                });
                for (name, &ty) in names.iter().zip(&shape.arguments).rev() {
                    let value = reference
                        .dict_get(name)
                        .ok_or_else(|| {
                            error(
                                RuntimeErrorKind::InvalidBytecode,
                                "codec field is absent from the solved record",
                                &function,
                                pc,
                            )
                        })?
                        .value;
                    state.pending.push(SolvedEncodeTask::Visit { value, ty });
                }
            }
            T::Option => {
                if reference.as_atom().is_some_and(|a| a.as_str() == "None") {
                    state.output.push(
                        Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), loc)
                            .with_type_id(crate::TypeId::solved(state.target)),
                    );
                } else {
                    let (tag, payload) = reference.tagged_parts().ok_or_else(|| {
                        error(
                            RuntimeErrorKind::InvalidBytecode,
                            "invalid solved Option payload",
                            &function,
                            pc,
                        )
                    })?;
                    if tag.as_atom().is_none_or(|a| a.as_str() != "Some") {
                        return Err(error(
                            RuntimeErrorKind::InvalidBytecode,
                            "invalid solved Option tag",
                            &function,
                            pc,
                        ));
                    }
                    state.pending.push(SolvedEncodeTask::Visit {
                        value: payload.value,
                        ty: shape.arguments[0],
                    });
                }
            }
            T::Nominal(symbol) => {
                let definition = types.definition(*symbol).ok_or_else(|| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        "codec nominal definition is missing",
                        &function,
                        pc,
                    )
                })?;
                let layout = types.layout(ty).ok_or_else(|| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        "codec nominal layout is missing",
                        &function,
                        pc,
                    )
                })?;
                match definition.operation {
                    TypeOperation::Struct => {
                        let names = definition
                            .members
                            .iter()
                            .map(|m| {
                                if rename {
                                    lower_camel_case(&m.name)
                                } else {
                                    m.name.clone()
                                }
                            })
                            .collect::<Vec<_>>();
                        if names
                            .iter()
                            .collect::<std::collections::BTreeSet<_>>()
                            .len()
                            != names.len()
                        {
                            return Err(error(
                                RuntimeErrorKind::TypeMismatch,
                                "duplicate external field name",
                                &function,
                                pc,
                            ));
                        }
                        state.pending.push(SolvedEncodeTask::Object { names, loc });
                        for (member, &ty) in definition.members.iter().zip(&layout.members).rev() {
                            let value = reference
                                .dict_get(&member.name)
                                .ok_or_else(|| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        "codec struct field is missing",
                                        &function,
                                        pc,
                                    )
                                })?
                                .value;
                            state.pending.push(SolvedEncodeTask::Visit {
                                value,
                                ty: ty.expect("struct member"),
                            });
                        }
                    }
                    TypeOperation::Newtype => {
                        let value = reference
                            .sequence_get(0)
                            .ok_or_else(|| {
                                error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "codec newtype payload is missing",
                                    &function,
                                    pc,
                                )
                            })?
                            .value;
                        state.pending.push(SolvedEncodeTask::Visit {
                            value,
                            ty: layout.members[0].expect("newtype payload"),
                        });
                    }
                    TypeOperation::Enum => {
                        if rename
                            && definition
                                .members
                                .iter()
                                .map(|m| lower_camel_case(&m.name))
                                .collect::<std::collections::BTreeSet<_>>()
                                .len()
                                != definition.members.len()
                        {
                            return Err(error(
                                RuntimeErrorKind::TypeMismatch,
                                "duplicate external variant name",
                                &function,
                                pc,
                            ));
                        }
                        if untagged && rename {
                            return Err(error(
                                RuntimeErrorKind::TypeMismatch,
                                "rename_all is not meaningful on an untagged Enum",
                                &function,
                                pc,
                            ));
                        }
                        if untagged && layout.members.iter().filter(|ty| ty.is_none()).count() > 1 {
                            return Err(error(
                                RuntimeErrorKind::TypeMismatch,
                                "untagged Enum may contain at most one unit variant",
                                &function,
                                pc,
                            ));
                        }
                        let (tag, payload) = if let Some((tag, payload)) = reference.tagged_parts()
                        {
                            (tag.as_atom(), Some(payload.value))
                        } else {
                            (reference.as_atom(), None)
                        };
                        let index = tag
                            .as_ref()
                            .and_then(|tag| {
                                definition
                                    .members
                                    .iter()
                                    .position(|m| m.name == tag.as_str())
                            })
                            .ok_or_else(|| {
                                error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "codec enum tag does not match its solved type",
                                    &function,
                                    pc,
                                )
                            })?;
                        let name = &definition.members[index].name;
                        let name = if rename {
                            lower_camel_case(name)
                        } else {
                            name.clone()
                        };
                        match (payload, layout.members[index]) {
                            (Some(value), Some(ty)) => {
                                if !untagged {
                                    state.pending.push(SolvedEncodeTask::Object {
                                        names: vec![name],
                                        loc,
                                    });
                                }
                                state.pending.push(SolvedEncodeTask::Visit { value, ty });
                            }
                            (None, None) if untagged => state.output.push(
                                Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), loc)
                                    .with_type_id(crate::TypeId::solved(state.target)),
                            ),
                            (None, None) => {
                                charge_allocation(account, name.len() as u64, &function, pc)?;
                                let payload =
                                    Val::new(current.string(Some(background), &name), loc);
                                state.output.push(solved_codec_tag(
                                    "String",
                                    payload,
                                    state.target,
                                    loc,
                                    current,
                                    background,
                                    account,
                                    &function,
                                    pc,
                                )?);
                            }
                            _ => {
                                return Err(error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "codec enum payload does not match its solved type",
                                    &function,
                                    pc,
                                ));
                            }
                        }
                    }
                    _ => {
                        return Err(error(
                            RuntimeErrorKind::InvalidBytecode,
                            "invalid nominal codec skeleton",
                            &function,
                            pc,
                        ));
                    }
                }
            }
            T::Function => {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "Function has no JSON codec",
                    &function,
                    pc,
                ));
            }
            T::Type | T::TypeOf => {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "cannot encode Type",
                    &function,
                    pc,
                ));
            }
            _ => {
                return Err(error(
                    RuntimeErrorKind::InvalidBytecode,
                    format!(
                        "solved codec encode does not support {:?} yet",
                        shape.constructor
                    ),
                    &function,
                    pc,
                ));
            }
        }
    }
    if state.output.len() != 1 {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "codec output stack is not closed",
            &state.function,
            state.pc,
        ));
    }
    Ok(VmAction::Return {
        value: state.output.pop().unwrap(),
        return_target: state.return_target,
    })
}
