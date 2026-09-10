#[derive(Debug)]
enum SolvedParseTask {
    Check {
        owner: crate::mir::TypeId,
        argument: Val,
    },
    Visit {
        ty: crate::mir::TypeId,
        range: Option<std::ops::Range<usize>>,
        path: String,
    },
    Record {
        ty: crate::mir::TypeId,
        names: Vec<String>,
    },
    Some,
}

#[derive(Debug)]
struct SolvedStringParse {
    codec_input: Option<Val>,
    input: Val,
    property: crate::mir::TypeId,
    pending: Vec<SolvedParseTask>,
    output: Vec<Val>,
    return_target: ReturnTarget,
    trace_frame: RuntimeFrame,
    function: Arc<BytecodeFunction>,
    pc: usize,
}

#[derive(Debug)]
struct SolvedParseCheck(SolvedStringParse, crate::Loc);
impl NativeContinuation for SolvedParseCheck {
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
        let state = self.0;
        if let Some(blame) =
            solved_check_rejection(result, current, background, &state.function, state.pc)?
        {
            if let Some(input) = state.codec_input {
                return finish_codec_payload(
                    BuiltinAtom::Err,
                    CodecNode::Existing(blame),
                    input,
                    state.return_target,
                    &state.function,
                    state.pc,
                    current,
                    background,
                    account,
                );
            }
            return Err(solved_construction_blame(
                blame,
                self.1,
                current,
                background,
                &state.function,
                state.pc,
            )?);
        }
        continue_solved_string_parse(state, current, background, account)
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

impl NativeContinuation for SolvedStringParse {
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
        continue_solved_string_parse(*self, current, background, account)
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

fn run_solved_string_parse(
    arguments: &[Val],
    codec: Option<(Val, String)>,
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
        .expect("solved parse image");
    let property = solved_metadata_id(arguments[0], types, function, pc)?;
    let target = solved_metadata_id(arguments[1], types, function, pc)?;
    let input = arguments[2];
    propagate_direct_failure(&input, function, pc)?;
    let length = (ValueRef {
        value: input,
        view: HeapView {
            current,
            background: Some(background),
        },
    })
    .as_str()
    .ok_or_else(|| runtime_shallow_type_error("String", input, function, pc))?
    .len();
    continue_solved_string_parse(
        SolvedStringParse {
            codec_input: codec.as_ref().map(|(input, _)| *input),
            input,
            property,
            pending: vec![SolvedParseTask::Visit {
                ty: target,
                range: Some(0..length),
                path: codec.map_or_else(|| "$".into(), |(_, path)| path),
            }],
            output: vec![],
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

fn continue_solved_string_parse(
    mut state: SolvedStringParse,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    use crate::execution_graph::{EvaluationError, PropertyKey, Request};
    use crate::mir::{PropertySite, TypeConstructor as T};
    let types = background.solved_types.as_ref().expect("parse image");
    let graph = background.solved_graph.as_ref().expect("parse graph");
    let function = Arc::clone(&state.function);
    let pc = state.pc;
    let loc = state.input.loc();
    let mut rejection = None;
    while let Some(task) = state.pending.pop() {
        consume_fuel(account, &function, pc)?;
        let (ty, range, path) = match task {
            SolvedParseTask::Check { owner, argument } => {
                let Some(node) = graph.construction_check(owner, PropertySite::Type) else {
                    continue;
                };
                return run_solved_construction_check(
                    node,
                    argument,
                    ReturnTarget::Native(Box::new(SolvedParseCheck(
                        state,
                        graph.nodes()[node.index()].location,
                    ))),
                    &function,
                    pc,
                    current,
                    background,
                    account,
                );
            }
            SolvedParseTask::Some => {
                let payload = state.output.pop().expect("optional parse payload");
                state
                    .output
                    .push(solved_some(payload, current, account, &function, pc)?);
                continue;
            }
            SolvedParseTask::Record { ty, names } => {
                let values = state.output.split_off(state.output.len() - names.len());
                charge_allocation(
                    account,
                    logical_value_bytes(names.len())
                        .map_err(|e| allocation_error(e.message, &function, pc))?,
                    &function,
                    pc,
                )?;
                let value = current
                    .record_value(names.into_iter().zip(values))
                    .map_err(|e| {
                        error(
                            RuntimeErrorKind::InvalidBytecode,
                            e.to_string(),
                            &function,
                            pc,
                        )
                    })?
                    .with_loc(loc);
                state.pending.push(SolvedParseTask::Check {
                    owner: ty,
                    argument: value,
                });
                state.output.push(
                    if matches!(types.types[ty.index()].constructor, T::Nominal(_)) {
                        value.with_type_id(crate::TypeId::solved(ty))
                    } else {
                        value
                    },
                );
                continue;
            }
            SolvedParseTask::Visit { ty, range, path } => (ty, range, path),
        };
        let shape = &types.types[ty.index()];
        if shape.constructor == T::Option {
            if range.is_none() {
                state
                    .output
                    .push(Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), loc));
            } else {
                state.pending.push(SolvedParseTask::Some);
                state.pending.push(SolvedParseTask::Visit {
                    ty: shape.arguments[0],
                    range,
                    path,
                });
            }
            continue;
        }
        let Some(range) = range else {
            rejection = Some(format!("{path}: required capture is absent"));
            break;
        };
        let view = HeapView {
            current,
            background: Some(background),
        };
        let source = (ValueRef {
            value: state.input,
            view,
        })
        .as_str()
        .expect("parse String input");
        let text = source.as_str().get(range.clone()).ok_or_else(|| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                "invalid regex capture range",
                &function,
                pc,
            )
        })?;
        match shape.constructor {
            T::String => {
                let value = if range == (0..source.len()) {
                    state.input
                } else {
                    charge_allocation(account, text.len() as u64, &function, pc)?;
                    let text = text.to_owned();
                    Val::new(current.string(Some(background), &text), loc)
                };
                state.output.push(value);
            }
            T::Int => match text.parse::<i64>() {
                Ok(value) => state.output.push(Val::new(DecodedValue::Int(value), loc)),
                Err(_) => rejection = Some(format!("{path}: input is not a valid Int")),
            },
            T::Float => match text.parse::<f64>().ok().filter(|value| value.is_finite()) {
                Some(value) => state.output.push(Val::new(DecodedValue::Float(value), loc)),
                None => rejection = Some(format!("{path}: input is not a finite Float")),
            },
            _ => {
                let Some(node) = graph.property(PropertyKey {
                    owner: ty,
                    site: PropertySite::Type,
                    property: state.property,
                }) else {
                    rejection = Some(format!("{path}: type has no std/string.parse capability"));
                    break;
                };
                let property = match current
                    .solved_evaluation
                    .as_mut()
                    .expect("parse evaluation")
                    .request(node)
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
                                    "ParseBy has no compiled initializer",
                                    &function,
                                    pc,
                                )
                            })?;
                        state.pending.push(SolvedParseTask::Visit {
                            ty,
                            range: Some(range),
                            path,
                        });
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
                            rule_boundary: None,
                        });
                    }
                    Err(EvaluationError::Failed(failure)) => {
                        return Err(propagated_failure_error(failure.0, loc, &function, pc));
                    }
                    Err(EvaluationError::Cycle(path)) => {
                        return Err(error(
                            RuntimeErrorKind::UninitializedDefinition,
                            format!("cyclic ParseBy demand: {path:?}"),
                            &function,
                            pc,
                        ));
                    }
                    Err(e) => {
                        return Err(error(
                            RuntimeErrorKind::InvalidBytecode,
                            format!("invalid ParseBy demand: {e:?}"),
                            &function,
                            pc,
                        ));
                    }
                };
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                let regex = (ValueRef {
                    value: property,
                    view,
                })
                .dict_get("regex")
                .ok_or_else(|| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        "ParseBy has no regex",
                        &function,
                        pc,
                    )
                })?;
                let source = (ValueRef {
                    value: state.input,
                    view,
                })
                .as_str()
                .expect("parse source");
                let fields = crate::regex::solved_captures(
                    regex,
                    &source.as_str()[range.clone()],
                    ty,
                    state.property,
                    types,
                    graph,
                );
                match fields {
                    Ok(fields) => {
                        state.pending.push(SolvedParseTask::Record {
                            ty,
                            names: fields.iter().map(|(name, _, _)| name.clone()).collect(),
                        });
                        for (name, ty, capture) in fields.into_iter().rev() {
                            state.pending.push(SolvedParseTask::Visit {
                                ty,
                                range: capture.map(|capture| {
                                    range.start + capture.start..range.start + capture.end
                                }),
                                path: format!("{path}.{name}"),
                            });
                        }
                    }
                    Err(e) => rejection = Some(format!("{path}: {}", e.message)),
                }
            }
        }
        if rejection.is_some() {
            break;
        }
    }
    if let Some(message) = rejection {
        if let Some(input) = state.codec_input {
            let blame = decode_blame(message, vec![input], &function, pc, current, account)?;
            return finish_codec_payload(
                BuiltinAtom::Err,
                CodecNode::Existing(blame),
                input,
                state.return_target,
                &function,
                pc,
                current,
                background,
                account,
            );
        }
        return finish_codec_payload(
            BuiltinAtom::Err,
            CodecNode::String(message, loc),
            state.input,
            state.return_target,
            &function,
            pc,
            current,
            background,
            account,
        );
    }
    if state.output.len() != 1 {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "parse output stack is not closed",
            &function,
            pc,
        ));
    }
    finish_codec_payload(
        BuiltinAtom::Ok,
        CodecNode::Existing(state.output.pop().unwrap()),
        state.input,
        state.return_target,
        &function,
        pc,
        current,
        background,
        account,
    )
}
