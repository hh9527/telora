#[derive(Debug)]
enum SolvedDecodeTask {
    Check {
        owner: crate::mir::TypeId,
        site: crate::mir::PropertySite,
        argument: Val,
    },
    Untagged {
        value: Val,
        owner: crate::mir::TypeId,
        path: String,
        next: usize,
        output_start: usize,
        matches: Vec<Val>,
        failures: Vec<Val>,
        awaiting: bool,
    },
    Visit {
        value: Val,
        ty: crate::mir::TypeId,
        path: String,
    },
    Sequence {
        count: usize,
        tuple: bool,
        loc: Option<crate::Loc>,
    },
    Record {
        names: Vec<String>,
        owner: Option<crate::mir::TypeId>,
        loc: Option<crate::Loc>,
    },
    Dict {
        owner: crate::mir::TypeId,
        shape: crate::heap::ShapeId,
        count: usize,
        loc: Option<crate::Loc>,
    },
    Newtype {
        owner: crate::mir::TypeId,
        loc: Option<crate::Loc>,
    },
    Variant {
        owner: crate::mir::TypeId,
        variant: u32,
        name: String,
        loc: Option<crate::Loc>,
    },
    Some {
        loc: Option<crate::Loc>,
    },
}

#[derive(Debug)]
struct SolvedDecode {
    blame_rejection: Option<Val>,
    rejection: Option<(String, Val)>,
    pending: Vec<SolvedDecodeTask>,
    output: Vec<Val>,
    arguments: Vec<Val>,
    source: crate::mir::TypeId,
    property_ids: Vec<crate::mir::TypeId>,
    return_target: ReturnTarget,
    trace_frame: RuntimeFrame,
    function: Arc<BytecodeFunction>,
    pc: usize,
}

#[derive(Debug)]
struct SolvedDecodeCheck(SolvedDecode);
impl NativeContinuation for SolvedDecodeCheck {
    fn return_target(&self) -> &ReturnTarget {
        &self.0.return_target
    }
    fn trace_frame(&self) -> &RuntimeFrame {
        &self.0.trace_frame
    }
    fn resume(
        self: Box<Self>,
        result: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        let mut state = self.0;
        state.blame_rejection =
            solved_check_rejection(result, current, background, &state.function, state.pc)?;
        continue_solved_decode(state, current, background, account)
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
            return_target: self.0.return_target,
        })
    }
}

#[derive(Debug)]
struct SolvedDecodeParse {
    decoder: SolvedDecode,
}

impl NativeContinuation for SolvedDecodeParse {
    fn return_target(&self) -> &ReturnTarget {
        &self.decoder.return_target
    }
    fn trace_frame(&self) -> &RuntimeFrame {
        &self.decoder.trace_frame
    }
    fn resume(
        self: Box<Self>,
        result: Val,
        current: &mut Heap,
        background: &Heap,
        account: &mut QuotaAccount,
    ) -> Result<VmAction, RuntimeError> {
        let Self { mut decoder } = *self;
        let view = HeapView {
            current,
            background: Some(background),
        };
        let (tag, value) = (ValueRef {
            value: result,
            view,
        })
        .tagged_parts()
        .ok_or_else(|| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                "text parser did not return Result",
                &decoder.function,
                decoder.pc,
            )
        })?;
        if tag.as_atom().is_some_and(|tag| tag.as_str() == "Ok") {
            decoder.output.push(value.value);
        } else if tag.as_atom().is_some_and(|tag| tag.as_str() == "Err") {
            decoder.blame_rejection = Some(value.value);
        } else {
            return Err(error(
                RuntimeErrorKind::InvalidBytecode,
                "invalid text parser result tag",
                &decoder.function,
                decoder.pc,
            ));
        }
        continue_solved_decode(decoder, current, background, account)
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
            return_target: self.decoder.return_target,
        })
    }
}

impl NativeContinuation for SolvedDecode {
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
        continue_solved_decode(*self, current, background, account)
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

/// Decode directly into the current heap. The input remains a Value graph;
/// scalar payloads and dictionary shapes are shared by their original handles.
fn run_solved_codec_decode(
    arguments: &[Val],
    signature: Option<Val>,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    use crate::mir::TypeConstructor as T;
    let types = background
        .solved_types
        .as_ref()
        .expect("solved decode image");
    let signature = signature.ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "decode requires a compiled native signature",
            function,
            pc,
        )
    })?;
    let signature = solved_metadata_id(signature, types, function, pc)?;
    let signature = &types.types[signature.index()];
    let target = solved_metadata_id(arguments[1], types, function, pc)?;
    if signature.constructor != T::Function || signature.arguments.len() != 4 {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "invalid solved decode signature",
            function,
            pc,
        ));
    }
    let result = &types.types[signature.arguments[3].index()];
    if result.constructor != T::Result || result.arguments.first() != Some(&target) {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "decode result differs from its solved target",
            function,
            pc,
        ));
    }
    let source = signature.arguments[2];
    let property_ids = {
        let view = HeapView {
            current,
            background: Some(background),
        };
        let properties = ValueRef {
            value: arguments[0],
            view,
        };
        [
            "decode_by_parse",
            "encode_by_display",
            "json_rename_all",
            "json_untagged",
            "parse_by",
        ]
        .into_iter()
        .map(|name| {
            let property = properties.dict_get(name).ok_or_else(|| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    "decode property contract is missing a field",
                    function,
                    pc,
                )
            })?;
            solved_metadata_id(property.value, types, function, pc)
        })
        .collect::<Result<Vec<_>, _>>()?
    };
    continue_solved_decode(
        SolvedDecode {
            blame_rejection: None,
            rejection: None,
            pending: vec![SolvedDecodeTask::Visit {
                value: arguments[2],
                ty: target,
                path: "$".into(),
            }],
            output: vec![],
            arguments: arguments.to_vec(),
            source,
            property_ids,
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

fn continue_solved_decode(
    state: SolvedDecode,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    use crate::execution_graph::{EvaluationError, Request};
    use crate::mir::{TypeConstructor as T, TypeOperation};
    let SolvedDecode {
        mut blame_rejection,
        mut rejection,
        mut pending,
        mut output,
        arguments,
        source,
        property_ids,
        return_target,
        trace_frame,
        function: caller,
        pc,
    } = state;
    let function = caller.as_ref();
    let types = background
        .solved_types
        .as_ref()
        .expect("solved decode image");
    loop {
        // Only data mismatches unwind to an alternative boundary. VM failures
        // leave through Result/NativeContinuation and are never trial failures.
        if rejection.is_some() || blame_rejection.is_some() {
            if let Some(index) = pending
                .iter()
                .rposition(|task| matches!(task, SolvedDecodeTask::Untagged { .. }))
            {
                pending.truncate(index + 1);
            } else {
                break;
            }
        }
        let Some(task) = pending.pop() else { break };
        consume_fuel(account, function, pc)?;
        let (value, ty, path) = match task {
            SolvedDecodeTask::Check {
                owner,
                site,
                argument,
            } => {
                let Some(node) = background
                    .solved_graph
                    .as_ref()
                    .expect("decode graph")
                    .construction_check(owner, site)
                else {
                    continue;
                };
                let call_function = Arc::clone(&caller);
                let state = SolvedDecode {
                    blame_rejection,
                    rejection,
                    pending,
                    output,
                    arguments,
                    source,
                    property_ids,
                    return_target,
                    trace_frame,
                    function: caller,
                    pc,
                };
                return run_solved_construction_check(
                    node,
                    argument,
                    ReturnTarget::Native(Box::new(SolvedDecodeCheck(state))),
                    &call_function,
                    pc,
                    current,
                    background,
                    account,
                );
            }
            SolvedDecodeTask::Untagged {
                value,
                owner,
                path,
                mut next,
                output_start,
                mut matches,
                mut failures,
                awaiting,
            } => {
                if awaiting {
                    if let Some(blame) = blame_rejection.take() {
                        rejection = None;
                        failures.push(blame);
                    } else if let Some((message, subject)) = rejection.take() {
                        failures.push(decode_blame(message, vec![subject], function, pc, current, account)?);
                    } else {
                        matches.push(output.pop().expect("untagged candidate result"));
                    }
                    output.truncate(output_start);
                }
                let T::Nominal(symbol) = types.types[owner.index()].constructor else {
                    unreachable!()
                };
                let definition = types.definition(symbol).expect("untagged definition");
                let layout = types.layout(owner).expect("untagged layout");
                let mut scheduled = false;
                while next < layout.members.len() {
                    let index = next;
                    next += 1;
                    if let Some(ty) = layout.members[index] {
                        pending.push(SolvedDecodeTask::Untagged {
                            value,
                            owner,
                            path: path.clone(),
                            next,
                            output_start,
                            matches: std::mem::take(&mut matches),
                            failures: std::mem::take(&mut failures),
                            awaiting: true,
                        });
                        pending.push(SolvedDecodeTask::Variant {
                            owner,
                            variant: index as u32,
                            name: definition.members[index].name.clone(),
                            loc: value.loc(),
                        });
                        pending.push(SolvedDecodeTask::Visit {
                            value,
                            ty,
                            path: path.clone(),
                        });
                        scheduled = true;
                        break;
                    }
                    let view = HeapView {
                        current,
                        background: Some(background),
                    };
                    if (ValueRef { value, view })
                        .as_atom()
                        .is_some_and(|atom| atom.as_str() == "None")
                    {
                        matches.push(
                            Val::new(
                                current.atom(Some(background), &definition.members[index].name),
                                value.loc(),
                            )
                            .with_type_id(crate::TypeId::solved(owner)),
                        );
                    }
                }
                if !scheduled {
                    if matches.len() == 1 {
                        output.push(matches.pop().unwrap());
                    } else {
                        let mut subjects = vec![value];
                        let message = if matches.is_empty() {
                            let view = HeapView { current, background: Some(background) };
                            let mut messages = Vec::new();
                            for (index, failure) in failures.iter().enumerate() {
                                if let DecodedValue::Opaque(handle) = failure.value()
                                    && let Ok(Object::Opaque(blame)) = view.object(handle)
                                    && let Some(message) = blame.downcast_ref::<String>(&crate::core::blame_native_type()) {
                                    messages.push(message.clone());
                                    // Keep the first concrete rejection's
                                    // subjects, including nested field origins.
                                    if index == 0 { subjects = blame.traced.to_vec(); }
                                }
                            }
                            format!("{path}: value matches no untagged Enum variant ({})", messages.join("; "))
                        } else {
                            format!("{path}: value ambiguously matches multiple untagged Enum variants")
                        };
                        blame_rejection = Some(decode_blame(message, subjects, function, pc, current, account)?);
                    }
                }
                continue;
            }
            SolvedDecodeTask::Sequence { count, tuple, loc } => {
                let values = output.split_off(output.len() - count).into_boxed_slice();
                charge_allocation(
                    account,
                    logical_value_bytes(count)
                        .map_err(|e| allocation_error(e.message, function, pc))?,
                    function,
                    pc,
                )?;
                let value = if tuple {
                    DecodedValue::Tuple(current.allocate(Object::Tuple(values)))
                } else {
                    DecodedValue::Array(current.allocate(Object::Array(values)))
                };
                output.push(Val::new(value, loc));
                continue;
            }
            SolvedDecodeTask::Record { names, owner, loc } => {
                let values = output.split_off(output.len() - names.len());
                charge_allocation(
                    account,
                    logical_value_bytes(names.len())
                        .map_err(|e| allocation_error(e.message, function, pc))?,
                    function,
                    pc,
                )?;
                let value = current
                    .record_value(names.into_iter().zip(values))
                    .map_err(|e| {
                        error(
                            RuntimeErrorKind::InvalidBytecode,
                            e.to_string(),
                            function,
                            pc,
                        )
                    })?
                    .with_loc(loc);
                if let Some(owner) = owner {
                    pending.push(SolvedDecodeTask::Check {
                        owner,
                        site: crate::mir::PropertySite::Type,
                        argument: value,
                    });
                }
                output.push(if let Some(owner) = owner {
                    value.with_type_id(crate::TypeId::solved(owner))
                } else {
                    value
                });
                continue;
            }
            SolvedDecodeTask::Dict { owner, shape, count, loc } => {
                let values = output.split_off(output.len() - count).into_boxed_slice();
                charge_allocation(
                    account,
                    logical_value_bytes(count)
                        .map_err(|e| allocation_error(e.message, function, pc))?,
                    function,
                    pc,
                )?;
                output.push(Val::new(
                    DecodedValue::Dict(current.allocate(Object::Dict { shape, values })),
                    loc,
                ).with_type_id(crate::TypeId::solved(owner)));
                continue;
            }
            SolvedDecodeTask::Newtype { owner, loc } => {
                let payload = output.pop().expect("decoded newtype payload");
                pending.push(SolvedDecodeTask::Check {
                    owner,
                    site: crate::mir::PropertySite::Type,
                    argument: payload,
                });
                charge_allocation(
                    account,
                    logical_value_bytes(1)
                        .map_err(|e| allocation_error(e.message, function, pc))?,
                    function,
                    pc,
                )?;
                output.push(
                    Val::new(
                        DecodedValue::Tuple(
                            current.allocate(Object::Tuple(vec![payload].into_boxed_slice())),
                        ),
                        loc,
                    )
                    .with_type_id(crate::TypeId::solved(owner)),
                );
                continue;
            }
            SolvedDecodeTask::Variant {
                owner,
                variant,
                name,
                loc,
            } => {
                let payload = output.pop().expect("decoded variant payload");
                pending.push(SolvedDecodeTask::Check {
                    owner,
                    site: crate::mir::PropertySite::Variant(variant),
                    argument: payload,
                });
                output.push(solved_codec_tag(
                    &name, payload, owner, loc, current, background, account, function, pc,
                )?);
                continue;
            }
            SolvedDecodeTask::Some { loc } => {
                let payload = output.pop().expect("decoded optional payload");
                output.push(solved_some(payload, current, account, function, pc)?.with_loc(loc));
                continue;
            }
            SolvedDecodeTask::Visit { value, ty, path } => (value, ty, path),
        };
        propagate_direct_failure(&value, function, pc)?;
        if value.type_id().and_then(crate::TypeId::solved_id) != Some(source) {
            return Err(error(
                RuntimeErrorKind::InvalidBytecode,
                "decode input is not the statically selected Value",
                function,
                pc,
            ));
        }
        if ty == source {
            output.push(value);
            continue;
        }
        let mut rename = false;
        let mut untagged = false;
        let mut bridged = false;
        if matches!(types.types[ty.index()].constructor, T::Nominal(_)) {
            let graph = background.solved_graph.as_ref().expect("decode graph");
            let has = |index: usize| {
                graph
                    .property(crate::execution_graph::PropertyKey {
                        owner: ty,
                        site: crate::mir::PropertySite::Type,
                        property: property_ids[index],
                    })
                    .is_some()
            };
            bridged = has(0);
            if bridged != has(1) {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    "std/string.decode_by_parse and std/string.encode_by_display must be used together",
                    function,
                    pc,
                ));
            }
            for (index, &property) in property_ids.iter().enumerate() {
                if index == 4 || bridged && index >= 2 {
                    continue;
                }
                let Some(node) = graph.property(crate::execution_graph::PropertyKey {
                    owner: ty,
                    site: crate::mir::PropertySite::Type,
                    property,
                }) else {
                    continue;
                };
                let property = match request_solved(current, background, node)
                {
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
                                    "decode property has no compiled initializer",
                                    function,
                                    pc,
                                )
                            })?;
                        pending.push(SolvedDecodeTask::Visit { value, ty, path });
                        let continuation = DemandContinuation {
                            node,
                            trace_frame: trace_frame.clone(),
                            call_function: Arc::clone(&caller),
                            call_pc: pc,
                            return_target: ReturnTarget::Native(Box::new(SolvedDecode {
                                blame_rejection,
                                rejection,
                                pending,
                                output,
                                arguments,
                                source,
                                property_ids,
                                return_target,
                                trace_frame,
                                function: Arc::clone(&caller),
                                pc,
                            })),
                        };
                        return Ok(VmAction::Call {
                            callee,
                            arguments: vec![],
                            return_target: ReturnTarget::Native(Box::new(continuation)),
                            call_function: caller,
                            call_pc: pc,
                            rule_boundary: None,
                        });
                    }
                    Err(EvaluationError::Failed(failure)) => {
                        return Err(propagated_failure_error(
                            failure.0,
                            value.loc(),
                            function,
                            pc,
                        ));
                    }
                    Err(EvaluationError::Cycle(path)) => {
                        return Err(error(
                            RuntimeErrorKind::UninitializedDefinition,
                            format!("cyclic decode property demand: {path:?}"),
                            function,
                            pc,
                        ));
                    }
                    Err(e) => {
                        return Err(error(
                            RuntimeErrorKind::InvalidBytecode,
                            format!("invalid decode property demand: {e:?}"),
                            function,
                            pc,
                        ));
                    }
                };
                if index < 2 {
                    continue;
                }
                if index == 3 {
                    untagged = true;
                    continue;
                }
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
                        function,
                        pc,
                    ));
                }
                rename = true;
            }
        }
        let loc = value.loc();
        let view = HeapView {
            current,
            background: Some(background),
        };
        let reference = ValueRef { value, view };
        let (tag, payload) = if let Some((tag, payload)) = reference.tagged_parts() {
            (tag.as_atom().map(|a| a.as_str().to_owned()), Some(payload))
        } else {
            (reference.as_atom().map(|a| a.as_str().to_owned()), None)
        };
        let shape = &types.types[ty.index()];
        if bridged {
            if tag.as_deref() != Some("String") || payload.is_none() {
                rejection = Some((
                    format!("{path}: expected String text representation"),
                    value,
                ));
                continue;
            }
            let parse_arguments = [
                Val::new(DecodedValue::SolvedType(property_ids[4]), loc),
                Val::new(DecodedValue::SolvedType(ty), loc),
                payload.unwrap().value,
            ];
            let call_function = Arc::clone(&caller);
            let decoder = SolvedDecode {
                blame_rejection,
                rejection,
                pending,
                output,
                arguments,
                source,
                property_ids,
                return_target,
                trace_frame,
                function: caller,
                pc,
            };
            return run_solved_string_parse(
                &parse_arguments,
                Some((value, path)),
                ReturnTarget::Native(Box::new(SolvedDecodeParse { decoder })),
                &call_function,
                pc,
                current,
                background,
                account,
            );
        }
        let mismatch = |expected: &str| Some((format!("{path}: expected {expected}"), value));
        match &shape.constructor {
            T::Int | T::Float | T::String | T::Bytes => {
                let expected = match shape.constructor {
                    T::Int => "Int",
                    T::Float => "Float",
                    T::String => "String",
                    _ => "Bytes",
                };
                if tag.as_deref() == Some(expected) && payload.is_some() {
                    output.push(payload.unwrap().value.without_type_id());
                } else {
                    rejection = mismatch(expected);
                }
            }
            T::Bool => {
                if payload.is_none() && matches!(tag.as_deref(), Some("True" | "False")) {
                    output.push(value.without_type_id());
                } else {
                    rejection = mismatch("Bool");
                }
            }
            T::Option => {
                if payload.is_none() && tag.as_deref() == Some("None") {
                    output.push(Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), loc));
                } else {
                    pending.push(SolvedDecodeTask::Some { loc });
                    pending.push(SolvedDecodeTask::Visit {
                        value,
                        ty: shape.arguments[0],
                        path,
                    });
                }
            }
            T::Array | T::Tuple => {
                if tag.as_deref() != Some("Array") {
                    rejection = mismatch("Array");
                } else {
                    let payload = payload.expect("Value.Array payload");
                    let count = payload.sequence_len().ok_or_else(|| {
                        error(
                            RuntimeErrorKind::InvalidBytecode,
                            "malformed Value.Array",
                            function,
                            pc,
                        )
                    })?;
                    if shape.constructor == T::Tuple && count != shape.arguments.len() {
                        rejection = mismatch("tuple with the declared arity");
                    } else {
                        pending.push(SolvedDecodeTask::Sequence {
                            count,
                            tuple: shape.constructor == T::Tuple,
                            loc,
                        });
                        for index in (0..count).rev() {
                            pending.push(SolvedDecodeTask::Visit {
                                value: payload.sequence_get(index).unwrap().value,
                                ty: if shape.constructor == T::Array {
                                    shape.arguments[0]
                                } else {
                                    shape.arguments[index]
                                },
                                path: format!("{path}[{index}]"),
                            });
                        }
                    }
                }
            }
            T::Record(_) | T::Nominal(_) | T::Dict => {
                let mut fields = vec![];
                let mut owner = None;
                match &shape.constructor {
                    T::Record(names) => {
                        fields.extend(names.iter().cloned().zip(shape.arguments.iter().copied()))
                    }
                    T::Nominal(symbol) => {
                        let definition = types
                            .definition(*symbol)
                            .expect("nominal decode definition");
                        let layout = types.layout(ty).expect("nominal decode layout");
                        let external_name = |name: &str| {
                            if rename {
                                lower_camel_case(name)
                            } else {
                                name.to_owned()
                            }
                        };
                        if rename
                            && definition
                                .members
                                .iter()
                                .map(|m| external_name(&m.name))
                                .collect::<std::collections::BTreeSet<_>>()
                                .len()
                                != definition.members.len()
                        {
                            return Err(error(
                                RuntimeErrorKind::TypeMismatch,
                                "duplicate external member name",
                                function,
                                pc,
                            ));
                        }
                        match definition.operation {
                            TypeOperation::Struct => {
                                owner = Some(ty);
                                fields.extend(
                                    definition
                                        .members
                                        .iter()
                                        .zip(&layout.members)
                                        .map(|(m, t)| (m.name.clone(), t.expect("struct member"))),
                                );
                            }
                            TypeOperation::Newtype => {
                                pending.push(SolvedDecodeTask::Newtype { owner: ty, loc });
                                pending.push(SolvedDecodeTask::Visit {
                                    value,
                                    ty: layout.members[0].expect("newtype member"),
                                    path,
                                });
                                continue;
                            }
                            TypeOperation::Enum => {
                                if untagged {
                                    if rename {
                                        return Err(error(
                                            RuntimeErrorKind::TypeMismatch,
                                            "rename_all is not meaningful on an untagged Enum",
                                            function,
                                            pc,
                                        ));
                                    }
                                    pending.push(SolvedDecodeTask::Untagged {
                                        value,
                                        owner: ty,
                                        path,
                                        next: 0,
                                        output_start: output.len(),
                                        matches: vec![],
                                        failures: vec![],
                                        awaiting: false,
                                    });
                                    continue;
                                }
                                if matches!(tag.as_deref(), Some("LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime")) {
                                    // Temporal data already supplies the external
                                    // variant name. Its text still goes through
                                    // the declared payload decoder and checks.
                                    let selected = definition.members.iter().position(|member| {
                                        external_name(&member.name) == tag.as_deref().unwrap()
                                    });
                                    if let Some(index) = selected
                                        && let Some(payload_type) = layout.members[index]
                                        && let Some(payload) = payload.map(|value| value.value)
                                    {
                                        let input = solved_codec_tag("String", payload, source, loc,
                                            current, background, account, function, pc)?;
                                        pending.push(SolvedDecodeTask::Variant {
                                            owner: ty, variant: index as u32,
                                            name: definition.members[index].name.clone(), loc,
                                        });
                                        pending.push(SolvedDecodeTask::Visit { value: input, ty: payload_type, path });
                                    } else {
                                        rejection = mismatch("a declared enum variant");
                                    }
                                    continue;
                                }
                                let selected = if tag.as_deref() == Some("String") {
                                    payload
                                        .and_then(|p| p.as_str())
                                        .and_then(|name| {
                                            definition.members.iter().position(|m| {
                                                external_name(&m.name) == name.as_str()
                                            })
                                        })
                                        .map(|index| (index, None))
                                } else if tag.as_deref() == Some("Object") {
                                    payload.and_then(|p| {
                                        p.dict_fields().filter(|fields| fields.len() == 1).and_then(
                                            |fields| {
                                                definition
                                                    .members
                                                    .iter()
                                                    .position(|m| {
                                                        external_name(&m.name) == fields[0]
                                                    })
                                                    .map(|index| {
                                                        (
                                                            index,
                                                            p.dict_get(fields[0]).map(|v| v.value),
                                                        )
                                                    })
                                            },
                                        )
                                    })
                                } else {
                                    None
                                };
                                if let Some((index, input)) = selected {
                                    match (layout.members[index], input) {
                                        (Some(payload_type), Some(value)) => {
                                            pending.push(SolvedDecodeTask::Variant {
                                                owner: ty,
                                                variant: index as u32,
                                                name: definition.members[index].name.clone(),
                                                loc,
                                            });
                                            pending.push(SolvedDecodeTask::Visit {
                                                value,
                                                ty: payload_type,
                                                path,
                                            });
                                            continue;
                                        }
                                        (None, None) => {
                                            output.push(
                                                Val::new(
                                                    current.atom(
                                                        Some(background),
                                                        &definition.members[index].name,
                                                    ),
                                                    loc,
                                                )
                                                .with_type_id(crate::TypeId::solved(ty)),
                                            );
                                            continue;
                                        }
                                        _ => {}
                                    }
                                }
                                rejection = mismatch("a declared enum variant");
                            }
                            _ => {
                                return Err(error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "invalid nominal decode skeleton",
                                    function,
                                    pc,
                                ));
                            }
                        }
                    }
                    _ => {}
                }
                if rejection.is_none() {
                    if tag.as_deref() != Some("Object") {
                        rejection = mismatch("Object");
                    } else {
                        let payload = payload.expect("Value.Object payload");
                        if shape.constructor == T::Dict {
                            let DecodedValue::Dict(handle) = payload.value.value() else {
                                return Err(error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "malformed Value.Object",
                                    function,
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
                                    function,
                                    pc,
                                )
                            })?
                            else {
                                unreachable!()
                            };
                            pending.push(SolvedDecodeTask::Dict {
                                owner: ty,
                                shape: *dict_shape,
                                count: values.len(),
                                loc,
                            });
                            for (index, &value) in values.iter().enumerate().rev() {
                                pending.push(SolvedDecodeTask::Visit {
                                    value,
                                    ty: shape.arguments[0],
                                    path: format!("{path}[{index}]"),
                                });
                            }
                        } else {
                            let names = payload.dict_fields().ok_or_else(|| {
                                error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "malformed Value.Object",
                                    function,
                                    pc,
                                )
                            })?;
                            let wire_name = |name: &str| {
                                if rename {
                                    lower_camel_case(name)
                                } else {
                                    name.to_owned()
                                }
                            };
                            if let Some(name) = names.iter().find(|name| {
                                !fields.iter().any(|(field, _)| wire_name(field) == **name)
                            }) {
                                rejection = Some((
                                    format!("{path}.{name}: unknown field"),
                                    payload.dict_get(name).unwrap().value,
                                ));
                            } else {
                                pending.push(SolvedDecodeTask::Record {
                                    names: fields.iter().map(|(name, _)| name.clone()).collect(),
                                    owner,
                                    loc,
                                });
                                for (name, ty) in fields.into_iter().rev() {
                                    let name = wire_name(&name);
                                    let child = if let Some(child) = payload.dict_get(&name) {
                                        child.value
                                    } else if types.types[ty.index()].constructor == T::Option {
                                        Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), loc)
                                            .with_type_id(crate::TypeId::solved(source))
                                    } else {
                                        rejection =
                                            Some((format!("{path}.{name}: missing required field"), value));
                                        break;
                                    };
                                    pending.push(SolvedDecodeTask::Visit {
                                        value: child,
                                        ty,
                                        path: format!("{path}.{name}"),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            _ => {
                return Err(error(
                    RuntimeErrorKind::InvalidBytecode,
                    format!("solved decode does not support {:?} yet", shape.constructor),
                    function,
                    pc,
                ));
            }
        }
    }
    if let Some(blame) = blame_rejection {
        return finish_codec_payload(
            BuiltinAtom::Err,
            CodecNode::Existing(blame),
            arguments[2],
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    if let Some((message, input)) = rejection {
        return finish_decode_failure(
            CodecFailure {
                message,
                input: Some(input),
            },
            arguments[2],
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    if output.len() != 1 {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "decode output stack is not closed",
            function,
            pc,
        ));
    }
    finish_codec_payload(
        BuiltinAtom::Ok,
        CodecNode::Existing(output.pop().unwrap()),
        arguments[2],
        return_target,
        function,
        pc,
        current,
        background,
        account,
    )
}
