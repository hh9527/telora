// Checked casts validate representation, then refine nominal boundaries. Both
// traversals consume the sealed image; neither executes a type expression.
#[derive(Clone, Debug)]
struct CastVisit {
    value: Val,
    source: crate::mir::TypeId,
    target: crate::mir::TypeId,
    path: String,
}

#[derive(Debug)]
enum CastBuild {
    Array,
    Tuple,
    Record(Vec<String>),
    Tagged(Val),
}

#[derive(Debug)]
enum CastTask {
    Visit(CastVisit),
    Build {
        original: Val,
        target: crate::mir::TypeId,
        kind: CastBuild,
        children: Vec<Val>,
    },
    Check {
        owner: crate::mir::TypeId,
        site: crate::mir::PropertySite,
    },
}

#[derive(Debug)]
struct SolvedCast {
    root: CastVisit,
    validating: bool,
    pending: Vec<CastTask>,
    output: Vec<Val>,
    return_target: ReturnTarget,
    trace_frame: RuntimeFrame,
    function: Arc<BytecodeFunction>,
    pc: usize,
}

#[derive(Debug)]
struct SolvedCastCheck(SolvedCast, crate::Loc);
impl NativeContinuation for SolvedCastCheck {
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
            return Err(solved_construction_blame(
                blame,
                self.1,
                current,
                background,
                &state.function,
                state.pc,
            )?);
        }
        continue_solved_cast(state, current, background, account)
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

fn run_solved_cast(
    value: Val,
    source: crate::mir::TypeId,
    target: crate::mir::TypeId,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let root = CastVisit {
        value,
        source,
        target,
        path: "value".into(),
    };
    continue_solved_cast(
        SolvedCast {
            pending: vec![CastTask::Visit(root.clone())],
            root,
            validating: true,
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

fn continue_solved_cast(
    mut state: SolvedCast,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    use crate::mir::{PropertySite, TypeConstructor as T};
    let types = background.solved_types.as_ref().expect("cast image");
    let graph = background.solved_graph.as_ref().expect("cast graph");
    let function = Arc::clone(&state.function);
    let pc = state.pc;
    loop {
        let Some(task) = state.pending.pop() else {
            if state.validating {
                state.validating = false;
                state.output.clear();
                state.pending.push(CastTask::Visit(state.root.clone()));
                continue;
            }
            return finish_codec_payload(
                BuiltinAtom::Ok,
                CodecNode::Existing(state.output.pop().expect("cast result")),
                state.root.value,
                state.return_target,
                &function,
                pc,
                current,
                background,
                account,
            );
        };
        consume_fuel(account, &function, pc)?;
        let visit = match task {
            CastTask::Check { owner, site } => {
                if state.validating {
                    continue;
                }
                let Some(node) = graph.construction_check(owner, site) else {
                    continue;
                };
                let value = *state.output.last().expect("cast candidate");
                let argument = match types
                    .definition(match types.types[owner.index()].constructor {
                        T::Nominal(s) => s,
                        _ => unreachable!(),
                    })
                    .expect("cast definition")
                    .operation
                {
                    crate::mir::TypeOperation::Newtype => {
                        (ValueRef {
                            value,
                            view: HeapView {
                                current,
                                background: Some(background),
                            },
                        })
                        .sequence_get(0)
                        .expect("newtype payload")
                        .value
                    }
                    crate::mir::TypeOperation::Enum => {
                        (ValueRef {
                            value,
                            view: HeapView {
                                current,
                                background: Some(background),
                            },
                        })
                        .tagged_parts()
                        .expect("variant payload")
                        .1
                        .value
                    }
                    _ => value,
                };
                return run_solved_construction_check(
                    node,
                    argument,
                    ReturnTarget::Native(Box::new(SolvedCastCheck(
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
            CastTask::Build {
                original,
                target,
                kind,
                children,
            } => {
                let values = state.output.split_off(state.output.len() - children.len());
                let unchanged = values
                    .iter()
                    .zip(&children)
                    .all(|(a, b)| a.value() == b.value() && a.type_id() == b.type_id());
                let value = if state.validating || unchanged {
                    original
                } else {
                    charge_allocation(
                        account,
                        logical_value_bytes(
                            values.len() + usize::from(matches!(kind, CastBuild::Tagged(_))),
                        )
                        .map_err(|e| allocation_error(e.message, &function, pc))?,
                        &function,
                        pc,
                    )?;
                    match kind {
                        CastBuild::Array => Val::new(
                            DecodedValue::Array(
                                current.allocate(Object::Array(values.into_boxed_slice())),
                            ),
                            original.loc(),
                        ),
                        CastBuild::Tuple => Val::new(
                            DecodedValue::Tuple(
                                current.allocate(Object::Tuple(values.into_boxed_slice())),
                            ),
                            original.loc(),
                        ),
                        CastBuild::Tagged(tag) => Val::new(
                            DecodedValue::Tagged(current.allocate(Object::Tagged {
                                tag,
                                payload: values[0],
                            })),
                            original.loc(),
                        ),
                        CastBuild::Record(names) => current
                            .record_value(names.into_iter().zip(values))
                            .map_err(|e| {
                                error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    e.to_string(),
                                    &function,
                                    pc,
                                )
                            })?
                            .with_loc(original.loc()),
                    }
                };
                state
                    .output
                    .push(value.with_type_id(crate::TypeId::solved(target)));
                continue;
            }
            CastTask::Visit(visit) => visit,
        };
        let CastVisit {
            value,
            source,
            target,
            path,
        } = visit;
        propagate_direct_failure(&value, &function, pc)?;
        if source == target {
            // Exact static identity needs no witness rewrite, including scalar
            // leaves whose Val carries no explicit type stamp. Preserve handles
            // of parent containers when all their children are unchanged.
            state.output.push(value);
            continue;
        }
        let source_shape = &types.types[source.index()];
        let target_shape = &types.types[target.index()];
        let mut check = None;
        let target_body = if let T::Nominal(_) = target_shape.constructor {
            check = Some(PropertySite::Type);
            &types.types[types.layout(target).expect("cast layout").body.index()]
        } else {
            target_shape
        };
        let source_shape =
            if source_shape.constructor == T::Unchecked && source_shape.arguments[0] == target {
                &types.types[types
                    .layout(target)
                    .expect("unchecked cast layout")
                    .body
                    .index()]
            } else {
                source_shape
            };
        let view = HeapView {
            current,
            background: Some(background),
        };
        let input = ValueRef { value, view };
        let mut children = vec![];
        let mut mismatch = matches!(source_shape.constructor, T::Nominal(_) | T::Unchecked);
        let kind = match (&source_shape.constructor, &target_body.constructor) {
            (T::Record(_) | T::Dict, T::Record(names)) if !mismatch => {
                let actual = input.dict_fields().expect("cast record input");
                mismatch = actual.len() != names.len();
                for (index, name) in names.iter().enumerate() {
                    let Some(value) = input.dict_get(name) else {
                        mismatch = true;
                        break;
                    };
                    let source = match &source_shape.constructor {
                        T::Dict => source_shape.arguments[0],
                        T::Record(fields) => {
                            source_shape.arguments[fields
                                .iter()
                                .position(|field| field == name)
                                .expect("source field")]
                        }
                        _ => unreachable!(),
                    };
                    children.push(CastVisit {
                        value: value.value,
                        source,
                        target: target_body.arguments[index],
                        path: format!("{path}.{name}"),
                    });
                }
                CastBuild::Record(names.clone())
            }
            (T::Record(_) | T::Dict, T::Dict) if !mismatch => {
                let names = input
                    .dict_fields()
                    .expect("cast dict input")
                    .iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>();
                for name in &names {
                    let source = match &source_shape.constructor {
                        T::Dict => source_shape.arguments[0],
                        T::Record(fields) => {
                            source_shape.arguments
                                [fields.iter().position(|field| field == name).unwrap()]
                        }
                        _ => unreachable!(),
                    };
                    children.push(CastVisit {
                        value: input.dict_get(name).unwrap().value,
                        source,
                        target: target_body.arguments[0],
                        path: format!("{path}.{name}"),
                    });
                }
                CastBuild::Record(names)
            }
            (T::Array, T::Array) | (T::Tuple, T::Tuple | T::Newtype) => {
                let count = input.sequence_len().expect("cast sequence input");
                let array = target_body.constructor == T::Array;
                mismatch = !array && count != target_body.arguments.len();
                if !mismatch {
                    for index in 0..count {
                        children.push(CastVisit {
                            value: input.sequence_get(index).unwrap().value,
                            source: source_shape.arguments[if array { 0 } else { index }],
                            target: target_body.arguments[if array { 0 } else { index }],
                            path: format!("{path}[{index}]"),
                        });
                    }
                }
                if array {
                    CastBuild::Array
                } else {
                    CastBuild::Tuple
                }
            }
            (T::Option, T::Option) | (T::Result, T::Result) => {
                if let Some((tag, payload)) = input.tagged_parts() {
                    let index = usize::from(
                        source_shape.constructor == T::Result
                            && tag.as_atom().is_some_and(|tag| tag == "Err"),
                    );
                    children.push(CastVisit {
                        value: payload.value,
                        source: source_shape.arguments[index],
                        target: target_body.arguments[index],
                        path: format!("{path}.payload"),
                    });
                    CastBuild::Tagged(tag.value)
                } else if source_shape.constructor == T::Option {
                    state
                        .output
                        .push(value.with_type_id(crate::TypeId::solved(target)));
                    continue;
                } else {
                    unreachable!("Result representation")
                }
            }
            _ => {
                mismatch = true;
                CastBuild::Tuple
            }
        };
        if mismatch {
            return finish_codec_payload(
                BuiltinAtom::Err,
                CodecNode::String(
                    format!("{path}: representation does not match cast target"),
                    value.loc(),
                ),
                state.root.value,
                state.return_target,
                &function,
                pc,
                current,
                background,
                account,
            );
        }
        if let Some(site) = check {
            state.pending.push(CastTask::Check {
                owner: target,
                site,
            });
        }
        state.pending.push(CastTask::Build {
            original: value,
            target,
            kind,
            children: children.iter().map(|child| child.value).collect(),
        });
        state
            .pending
            .extend(children.into_iter().rev().map(CastTask::Visit));
    }
}
