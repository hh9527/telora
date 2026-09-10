impl Vm {
    /// Import the immutable type arena into Main before executing any bytecode.
    /// Data imports are materialized with their statically solved type IDs.
    pub fn execute_linked(
        &mut self,
        entry: crate::execution_link::LinkedEntry,
        quota: Quota,
        limits: crate::DataLimits,
        sources: &mut SourceDatabase,
    ) -> Result<crate::execution_link::SolvedExecution, String> {
        let mut main = Heap::main();
        main.solved_types = Some(entry.types);
        main.solved_graph = Some(entry.graph);
        let mut account = QuotaAccount::new(quota).with_data_limits(limits).with_sources(sources);
        let externals = solved_module_data(&mut main, entry.data, limits, sources, &mut account)?;
        let main = Arc::new(main);
        let work = self
            .execute_in_work(&main, &externals, &entry.bytecode, &[], &mut account)
            .map_err(|error| error.with_sources(sources).to_string())?;
        Ok(crate::execution_link::SolvedExecution {
            world: ExecutionWorld::new(main, work),
            result_type: entry.result_type,
        })
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_debug_sink(mut self, sink: Arc<dyn DebugSink>) -> Self {
        self.debug_sink = sink;
        self
    }

    pub fn execute(
        &mut self,
        function: &BytecodeFunction,
        evaluation_fuel: usize,
    ) -> Result<ExecutionWorld, RuntimeError> {
        self.execute_with_args(function, &[], evaluation_fuel)
    }

    pub fn execute_with_args(
        &mut self,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        evaluation_fuel: usize,
    ) -> Result<ExecutionWorld, RuntimeError> {
        self.execute_with_quota_and_args(function, arguments, Quota::with_fuel(evaluation_fuel))
    }

    pub fn execute_with_quota(
        &mut self,
        function: &BytecodeFunction,
        quota: Quota,
    ) -> Result<ExecutionWorld, RuntimeError> {
        self.execute_with_quota_and_args(function, &[], quota)
    }

    pub fn execute_with_quota_and_args(
        &mut self,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        quota: Quota,
    ) -> Result<ExecutionWorld, RuntimeError> {
        let mut account = QuotaAccount::new(quota);
        self.execute_with_account(function, arguments, &mut account)
    }

    pub(crate) fn execute_with_account(
        &mut self,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        account: &mut QuotaAccount,
    ) -> Result<ExecutionWorld, RuntimeError> {
        let diagnostic_start = account.diagnostics.len();
        let background = Arc::new(Heap::main());
        let arena = self.execute_frame(
            &background,
            &HashMap::new(),
            function,
            None,
            None,
            &[],
            arguments,
            &[],
            account,
        )?;
        fail_on_reported_error(account, diagnostic_start, function)?;
        Ok(ExecutionWorld::new(background, arena))
    }

    pub(crate) fn execute_in_work(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        account: &mut QuotaAccount,
    ) -> Result<WorkWorld, RuntimeError> {
        let diagnostic_start = account.diagnostics.len();
        let arena = self.execute_frame(
            background,
            externals,
            function,
            None,
            None,
            &[],
            arguments,
            &[],
            account,
        )?;
        fail_on_reported_error(account, diagnostic_start, function)?;
        Ok(arena)
    }

    pub(crate) fn execute_in_work_best_effort_with_failures(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        arguments: &[crate::DataWorld],
        account: &mut QuotaAccount,
        inherited_failure_count: usize,
    ) -> Result<VmExecution, VmExecutionFailure> {
        self.execute_frame_with_policy(
            background,
            externals,
            function,
            None,
            None,
            &[],
            arguments,
            &[],
            account,
            true,
            inherited_failure_count,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_in_existing_world_with_runtime_args(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        world: WorkWorld,
        runtime_arguments: &[Val],
        arguments: &[crate::DataWorld],
        account: &mut QuotaAccount,
    ) -> Result<WorkWorld, RuntimeError> {
        let diagnostic_start = account.diagnostics.len();
        let WorkWorld { heap, root } = world;
        let mut existing_arguments = Vec::with_capacity(runtime_arguments.len() + 1);
        existing_arguments.push(root);
        existing_arguments.extend_from_slice(runtime_arguments);
        let arena = self.execute_frame(
            background,
            externals,
            function,
            Some(heap),
            None,
            &existing_arguments,
            arguments,
            &[],
            account,
        )?;
        fail_on_reported_error(account, diagnostic_start, function)?;
        Ok(arena)
    }

    #[allow(clippy::needless_borrow, clippy::too_many_arguments)]
    fn execute_frame(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        initial_work: Option<Heap>,
        work_state: Option<WorkWorld>,
        existing_arguments: &[Val],
        arguments: &[crate::DataWorld],
        captures: &[crate::DataWorld],
        account: &mut QuotaAccount,
    ) -> Result<WorkWorld, RuntimeError> {
        self.execute_frame_with_policy(
            background,
            externals,
            function,
            initial_work,
            work_state,
            existing_arguments,
            arguments,
            captures,
            account,
            false,
            0,
            true,
        )
        .map(|execution| execution.world)
        .map_err(|failure| failure.error)
    }

    #[allow(clippy::needless_borrow, clippy::too_many_arguments)]
    fn execute_frame_with_policy(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        initial_work: Option<Heap>,
        work_state: Option<WorkWorld>,
        existing_arguments: &[Val],
        arguments: &[crate::DataWorld],
        captures: &[crate::DataWorld],
        account: &mut QuotaAccount,
        best_effort: bool,
        inherited_failure_count: usize,
        publish: bool,
    ) -> Result<VmExecution, VmExecutionFailure> {
        // Linking recursively walks the immutable prototype graph. Keep that host
        // recursion off callers' often-small test or embedding threads; VM calls
        // themselves use the explicit frame stack below.
        let mut current = initial_work.unwrap_or_else(|| Heap::work_for(background));
        if current.solved_evaluation.is_none() && let Some(graph) = &background.solved_graph {
            current.solved_evaluation = Some(graph.evaluation());
            current.solved_tasks.resize(graph.nodes().len(), None);
        }
        let linked = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("telora-bytecode-linker".into())
                .stack_size(16 * 1024 * 1024)
                .spawn_scoped(scope, || {
                    current.link_bytecode_resolved(Some(background), function, externals)
                })
                .map_err(|_| crate::heap::HeapError::new("failed to start bytecode linker"))
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    )
                })?
                .join()
                .map_err(|_| crate::heap::HeapError::new("bytecode linker panicked"))
                .map_err(|heap_error| {
                    error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    )
                })
        });
        let prototype = match linked {
            Ok(prototype) => prototype,
            Err(error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error,
                    failures: Vec::new(),
                });
            }
        };
        let prototype = match prototype {
            Ok(prototype) => prototype,
            Err(heap_error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error: error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    ),
                    failures: Vec::new(),
                });
            }
        };
        let mut runtime_arguments = Vec::with_capacity(
            existing_arguments.len() + arguments.len() + usize::from(work_state.is_some()),
        );
        runtime_arguments.extend_from_slice(existing_arguments);
        if let Some(WorkWorld { heap, root }) = work_state {
            let relocated = match relocate_work_roots(&mut current, background, &heap, &[root]) {
                Ok(relocated) => relocated,
                Err(heap_error) => {
                    return Err(VmExecutionFailure {
                        heap: current,
                        error: error(
                            RuntimeErrorKind::InvalidBytecode,
                            heap_error.to_string(),
                            function,
                            0,
                        ),
                        failures: Vec::new(),
                    });
                }
            };
            runtime_arguments.extend(relocated);
        }
        let imported_arguments = arguments
            .iter()
            .map(|value| value.relocate_into(&mut current, background))
            .collect::<Result<Vec<_>, _>>();
        let imported_arguments = match imported_arguments {
            Ok(arguments) => arguments,
            Err(heap_error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error: error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    ),
                    failures: Vec::new(),
                });
            }
        };
        runtime_arguments.extend(imported_arguments);
        let captures = captures
            .iter()
            .map(|value| value.relocate_into(&mut current, background))
            .collect::<Result<Vec<_>, _>>();
        let captures = match captures {
            Ok(captures) => captures,
            Err(heap_error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error: error(
                        RuntimeErrorKind::InvalidBytecode,
                        heap_error.to_string(),
                        function,
                        0,
                    ),
                    failures: Vec::new(),
                });
            }
        };
        let mut stack: Vec<Option<Val>> = Vec::new();
        let root_frame = make_execution_frame(
            Arc::new(function.clone()),
            prototype,
            &runtime_arguments,
            &captures,
            ReturnTarget::Root,
            None,
            &mut stack,
            account.stack_limit(),
        );
        let root_frame = match root_frame {
            Ok(frame) => frame,
            Err(error) => {
                return Err(VmExecutionFailure {
                    heap: current,
                    error,
                    failures: Vec::new(),
                });
            }
        };
        let mut frames = vec![root_frame];
        let debug_sink = Arc::clone(&self.debug_sink);

        // A failed node may arrive through an imported Main-world Module. Its
        // id is below the stable prefix length owned by that Main world; only
        // newly created roots need to be retained by this execution.
        let mut failures = Vec::new();
        let mut result = (|| -> Result<Val, RuntimeError> {
            loop {
                let attempt = (|| -> Result<Val, RuntimeError> {
                    loop {
                        let function_arc = frames
                            .last()
                            .expect("execution has at least one frame")
                            .function
                            .clone();
                        let function = function_arc.as_ref();
                        let pc = frames.last().expect("execution frame").pc;
                        let tail_return = frames.last().expect("execution frame").tail_return
                            .map(|src| Opcode::Return { src });
                        let instruction = tail_return.as_ref().or_else(|| function.instructions().get(pc)).ok_or_else(|| {
                            error(
                                RuntimeErrorKind::InvalidBytecode,
                                "instruction pointer is out of bounds",
                                function,
                                pc,
                            )
                        })?;
                        let frame = frames.last().expect("execution frame");
                        let base = frame.base;
                        let end = base + frame.function.register_count();
                        let mut registers = &mut stack[base..end];
                        let view = WorkView {
                            main: background,
                            work: &current,
                        }
                        .heap_view();

                        match instruction {
                            Opcode::LoadConst { dst, value } => {
                                let (_, values, _, _) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let value = values.get(value.0).copied().ok_or_else(|| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        format!("value link {} is out of bounds", value.0),
                                        function,
                                        pc,
                                    )
                                })?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    value.with_loc(
                                        value.loc().or(instruction_location(function, pc)),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::StampType { dst, src, ty } => {
                                let value = *read_register(&registers, *src, function, pc)?;
                                write_register(&mut registers, *dst, value.with_type_id(crate::TypeId::solved(*ty)), function, pc)?;
                            }
                            Opcode::Move { dst, src } => {
                                let value = *read_register(&registers, *src, function, pc)?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::InstallTask { node, src } => {
                                let value = *read_register(&registers, *src, function, pc)?;
                                let slot = current.solved_tasks.get_mut(node.index()).ok_or_else(|| error(
                                    RuntimeErrorKind::InvalidBytecode, "invalid demand task slot", function, pc))?;
                                if slot.is_some() { return Err(error(RuntimeErrorKind::InvalidBytecode,
                                    "demand task installed twice", function, pc)); }
                                if !matches!(value.value(), DecodedValue::Func(_)) { return Err(error(
                                    RuntimeErrorKind::InvalidBytecode, "demand task must be a function", function, pc)); }
                                *slot = Some(value);
                            }
                            Opcode::CheckedCast { dst, src, source, target } => {
                                let value = *read_register(&registers, *src, function, pc)?;
                                let action = run_solved_cast(value, *source, *target,
                                    ReturnTarget::Register { destination: *dst, call_site: instruction_location(function, pc) },
                                    function, pc, &mut current, background, account)?;
                                frames.last_mut().expect("cast caller").pc += 1;
                                let _ = registers;
                                match drive_vm_action(action, &mut frames, &mut stack, &mut current, background, account)? {
                                    DriveOutcome::Pending => continue,
                                    DriveOutcome::Root(value) => return Ok(value),
                                }
                            }
                            Opcode::MakeSome { dst, value } => {
                                let value = *read_register(&registers, *value, function, pc)?;
                                let value = solved_some(value, &mut current, account, function, pc)?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::Demand { .. } | Opcode::GetTypeProp { .. } | Opcode::GetMemberProp { .. } | Opcode::HasTypeProp { .. } | Opcode::HasMemberProp { .. } => {
                                use crate::execution_graph::{Request, EvaluationError};
                                let (node_id, destination) = match instruction {
                                    Opcode::Demand { node, dst } => (*node, *dst),
                                    Opcode::GetTypeProp { dst, owner, property } | Opcode::GetMemberProp { dst, owner, property, .. } | Opcode::HasTypeProp { dst, owner, property } | Opcode::HasMemberProp { dst, owner, property, .. } => {
                                        let site = if let Opcode::GetMemberProp { index, variant, .. } | Opcode::HasMemberProp { index, variant, .. } = instruction {
                                            let index = read_register(&registers, *index, function, pc)?;
                                            let DecodedValue::Int(index) = index.value() else {
                                                return Err(error(RuntimeErrorKind::TypeMismatch, "property member index must be Int", function, pc));
                                            };
                                            let index = u32::try_from(index).map_err(|_| error(RuntimeErrorKind::TypeMismatch, "property member index must fit u32", function, pc))?;
                                            if *variant { crate::mir::PropertySite::Variant(index) } else { crate::mir::PropertySite::Field(index) }
                                        } else { crate::mir::PropertySite::Type };
                                        let owner = *read_register(&registers, *owner, function, pc)?;
                                        let property = *read_register(&registers, *property, function, pc)?;
                                        let (DecodedValue::SolvedType(owner), DecodedValue::SolvedType(property)) = (owner.value(), property.value()) else {
                                            return Err(error(RuntimeErrorKind::TypeMismatch, "property query requires solved Type metadata", function, pc));
                                        };
                                        let graph = background.solved_graph.as_ref().ok_or_else(|| error(RuntimeErrorKind::InvalidBytecode, "property query requires a solved graph", function, pc))?;
                                        let found = graph.property(crate::execution_graph::PropertyKey { owner, property, site });
                                        if matches!(instruction, Opcode::HasTypeProp { .. } | Opcode::HasMemberProp { .. }) {
                                            let value = if found.is_some() { crate::BuiltinAtom::True } else { crate::BuiltinAtom::False };
                                            write_register(&mut registers, *dst, Val::unknown(DecodedValue::BuiltinAtom(value)), function, pc)?;
                                            frames.last_mut().expect("query frame").pc += 1;
                                            continue;
                                        }
                                        let node = found.ok_or_else(|| error(RuntimeErrorKind::InvalidBytecode, "property read requires a proven presence record", function, pc))?;
                                        (node, *dst)
                                    }
                                    _ => unreachable!(),
                                };
                                let (node, dst) = (&node_id, &destination);
                                let state = current.solved_evaluation.as_mut().ok_or_else(|| error(
                                    RuntimeErrorKind::InvalidBytecode, "demand instruction requires a solved session", function, pc))?;
                                match state.request(*node) {
                                    Ok(Request::Ready(value)) => {
                                        let value = *value;
                                        write_register(&mut registers, *dst, value, function, pc)?;
                                    }
                                    Err(EvaluationError::Failed(failure)) => {
                                        if current.solved_failures.get(failure.0 as usize).is_none() {
                                            return Err(error(RuntimeErrorKind::InvalidBytecode, "demand failure has no session diagnostic", function, pc));
                                        }
                                        return Err(propagated_failure_error(failure.0, instruction_location(function, pc), function, pc));
                                    }
                                    Err(EvaluationError::Cycle(path)) => {
                                        let path = path.iter().map(|n| background.solved_graph.as_ref()
                                            .and_then(|g| g.nodes().get(n.index())).map(|n| n.label.clone())
                                            .unwrap_or_else(|| n.index().to_string())).collect::<Vec<_>>().join(" -> ");
                                        return Err(error(RuntimeErrorKind::UninitializedDefinition,
                                            format!("cyclic demand: {path}"), function, pc));
                                    }
                                    Err(e) => return Err(error(RuntimeErrorKind::InvalidBytecode, format!("invalid demand: {e:?}"), function, pc)),
                                    Ok(Request::Start) => {
                                        let callee = current.solved_tasks.get(node.index()).copied().flatten().ok_or_else(|| error(
                                            RuntimeErrorKind::InvalidBytecode, "demand task has no compiled initializer", function, pc))?;
                                        let continuation = DemandContinuation {
                                            node: *node,
                                            return_target: ReturnTarget::Register { destination: *dst, call_site: instruction_location(function, pc) },
                                            trace_frame: RuntimeFrame { function: function.name().to_owned(), instruction: pc, origin: function.origin_at(pc) },
                                            call_function: Arc::clone(&function_arc), call_pc: pc,
                                        };
                                        frames.last_mut().expect("caller frame").pc += 1;
                                        let _ = registers;
                                        match drive_vm_action(VmAction::Call {
                                            callee, arguments: vec![], return_target: ReturnTarget::Native(Box::new(continuation)),
                                            call_function: function_arc, call_pc: pc, rule_boundary: None,
                                        }, &mut frames, &mut stack, &mut current, background, account)? {
                                            DriveOutcome::Pending => continue,
                                            DriveOutcome::Root(value) => return Ok(value),
                                        }
                                    }
                                }
                            }
                            Opcode::MakeNewtype { dst, ty, payload } => {
                                let types = background.solved_types.as_ref().ok_or_else(|| error(RuntimeErrorKind::InvalidBytecode, "newtype requires a solved image", function, pc))?;
                                let definition = types.types.get(ty.index()).and_then(|ty| {
                                    if let crate::mir::TypeConstructor::Nominal(symbol) = ty.constructor { types.definition(symbol) } else { None }
                                });
                                if definition.is_none_or(|d| d.operation != crate::mir::TypeOperation::Newtype || d.members.len() != 1) {
                                    return Err(error(RuntimeErrorKind::InvalidBytecode, "invalid solved newtype constructor", function, pc));
                                }
                                let payload = *read_register(&registers, *payload, function, pc)?;
                                charge_allocation(account, logical_value_bytes(1).map_err(|e| allocation_error(e.message, function, pc))?, function, pc)?;
                                let value = Val::unknown(DecodedValue::Tuple(current.allocate(Object::Tuple(vec![payload].into_boxed_slice()))))
                                    .with_type_id(crate::TypeId::solved(*ty)).with_loc(instruction_location(function, pc));
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::MakeVariant { dst, ty, variant, payload } => {
                                if let Some((tag, has_payload)) = background.solved_types.as_ref()
                                    .and_then(|types| types.types.get(ty.index()))
                                    .and_then(|ty| crate::type_image::builtin_variant(&ty.constructor, *variant)) {
                                    if has_payload != payload.is_some() {
                                        return Err(error(RuntimeErrorKind::InvalidBytecode, "invalid native variant payload", function, pc));
                                    }
                                    let tag = Val::unknown(current.atom(Some(background), tag));
                                    let value = if let Some(src) = payload {
                                        let payload = *read_register(&registers, *src, function, pc)?;
                                        charge_allocation(account, logical_value_bytes(2).map_err(|e| allocation_error(e.message, function, pc))?, function, pc)?;
                                        Val::unknown(DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })))
                                    } else { tag };
                                    let value = if background.solved_types.as_ref().unwrap().types[ty.index()].constructor == crate::mir::TypeConstructor::PropertyTarget {
                                        value.with_type_id(crate::TypeId::solved(*ty))
                                    } else { value };
                                    write_register(&mut registers, *dst, value.with_loc(instruction_location(function, pc)), function, pc)?;
                                    frames.last_mut().expect("variant frame").pc += 1;
                                    continue;
                                }
                                let member = background.solved_types.as_ref()
                                    .and_then(|types| types.variant(*ty, *variant))
                                    .filter(|member| member.payload.is_some() == payload.is_some())
                                    .ok_or_else(|| error(RuntimeErrorKind::InvalidBytecode,
                                        "invalid solved enum constructor", function, pc))?;
                                let bytes = logical_value_bytes(if payload.is_some() { 2 } else { 0 })
                                    .map_err(|e| allocation_error(e.message, function, pc))?;
                                charge_allocation(account, bytes + member.name.len() as u64, function, pc)?;
                                let tag = Val::unknown(current.atom(Some(background), &member.name));
                                let value = if let Some(src) = payload {
                                    let payload = *read_register(&registers, *src, function, pc)?;
                                    Val::unknown(DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })))
                                } else { tag };
                                let value = value.with_type_id(crate::TypeId::solved(*ty))
                                    .with_loc(instruction_location(function, pc));
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::OwnDeclared { dst, owner, value } => {
                                let owner = *read_register(&registers, *owner, function, pc)?;
                                let value = *read_register(&registers, *value, function, pc)?;
                                let type_id =
                                    view.declared_type_id(owner).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                if let Some(action) = construction_check_action(
                                    owner, value, type_id,
                                    || ReturnTarget::Register { destination: *dst, call_site: None },
                                    false,
                                    Arc::clone(&function_arc), pc, &mut current, background, account,
                                )? {
                                    frames.last_mut().expect("construction frame").pc += 1;
                                    let _ = registers;
                                    match drive_vm_action(action, &mut frames, &mut stack,
                                        &mut current, background, account)? {
                                        DriveOutcome::Pending => continue,
                                        DriveOutcome::Root(value) => return Ok(value),
                                    }
                                }
                                write_register(
                                    &mut registers,
                                    *dst,
                                    value.with_type_id(type_id),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::AllocFunc { dst, static_id } => {
                                let value = if let Some(id) = static_id {
                                    Val::new(
                                        DecodedValue::FuncRef(*id),
                                        instruction_location(function, pc),
                                    )
                                } else {
                                    charge_allocation(
                                        account,
                                        logical_value_bytes(1).map_err(|native_error| {
                                            allocation_error(native_error.message, function, pc)
                                        })?,
                                        function,
                                        pc,
                                    )?;
                                    Val::new(
                                        DecodedValue::Func(
                                            current.allocate(crate::heap::Object::OpenFunc),
                                        ),
                                        instruction_location(function, pc),
                                    )
                                };
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::SealFunc { target, source } => {
                                let target = *read_register(&registers, *target, function, pc)?;
                                let source = *read_register(&registers, *source, function, pc)?;
                                if view
                                    .resolve_func(source)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .is_none()
                                {
                                    return Err(error(
                                        RuntimeErrorKind::TypeMismatch,
                                        "function definition did not produce a FuncRef",
                                        function,
                                        pc,
                                    ));
                                }
                                match target.value() {
                                    DecodedValue::Func(target) => {
                                        let DecodedValue::Func(source) = source.value() else {
                                            return Err(error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                "dynamic function slot cannot retain a static reference",
                                                function,
                                                pc,
                                            ));
                                        };
                                        current.seal_local_func(target, source).map_err(
                                            |heap_error| {
                                                error(
                                                    RuntimeErrorKind::DuplicateDefinition,
                                                    heap_error.to_string(),
                                                    function,
                                                    pc,
                                                )
                                            },
                                        )?;
                                    }
                                    DecodedValue::FuncRef(id) => current
                                        .seal_static_func(id, source)
                                        .map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::DuplicateDefinition,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?,
                                    _ => {
                                        return Err(error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            "function ref target is not a FuncRef",
                                            function,
                                            pc,
                                        ));
                                    }
                                }
                            }
                            Opcode::AllocTypeSlot { dst } => {
                                charge_allocation(
                                    account,
                                    logical_value_bytes(1).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?,
                                    function,
                                    pc,
                                )?;
                                let link =
                                    Val::new(
                                        DecodedValue::TypeSlot(current.allocate(
                                            crate::heap::Object::TypeSlot { value: None },
                                        )),
                                        instruction_location(function, pc),
                                    );
                                write_register(&mut registers, *dst, link, function, pc)?;
                            }
                            Opcode::ReadTypeSlot { dst, link } => {
                                let DecodedValue::TypeSlot(handle) =
                                    read_register(&registers, *link, function, pc)?.value()
                                else {
                                    return Err(error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        "up-link read operand is not an up-link",
                                        function,
                                        pc,
                                    ));
                                };
                                let value = view
                                    .type_slot(handle)
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
                                            RuntimeErrorKind::UninitializedDefinition,
                                            "definition was read before initialization",
                                            function,
                                            pc,
                                        )
                                    })?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::SealTypeSlot { link, src } => {
                                let DecodedValue::TypeSlot(handle) =
                                    read_register(&registers, *link, function, pc)?.value()
                                else {
                                    return Err(error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        "up-link initialization operand is not an up-link",
                                        function,
                                        pc,
                                    ));
                                };
                                if view
                                    .type_slot(handle)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .is_some()
                                {
                                    return Err(error(
                                        RuntimeErrorKind::DuplicateDefinition,
                                        "definition was initialized more than once",
                                        function,
                                        pc,
                                    ));
                                }
                                let value = *read_register(&registers, *src, function, pc)?;
                                current.initialize_type_slot(handle, value).map_err(
                                    |heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    },
                                )?;
                            }
                            Opcode::AssertTypeSlotReady { link } => {
                                let DecodedValue::TypeSlot(handle) =
                                    read_register(&registers, *link, function, pc)?.value()
                                else {
                                    return Err(error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        "up-link assertion operand is not an up-link",
                                        function,
                                        pc,
                                    ));
                                };
                                if view
                                    .type_slot(handle)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .is_none()
                                {
                                    return Err(error(
                                        RuntimeErrorKind::UninitializedDefinition,
                                        "declaration was not initialized before block completion",
                                        function,
                                        pc,
                                    ));
                                }
                            }
                            Opcode::Add { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Add,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    value.with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::Subtract { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Subtract,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    value.with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::Multiply { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Multiply,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::Divide { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Divide,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::Remainder { dst, left, right } => {
                                let value = numeric_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    NumericOperation::Remainder,
                                    &view,
                                    account,
                                    function,
                                    pc,
                                )?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::Negate { dst, src } => {
                                let input = *read_register(&registers, *src, function, pc)?;
                                let value = match input.value() {
                                    DecodedValue::Int(value) => {
                                        DecodedValue::Int(value.checked_neg().ok_or_else(|| {
                                            error(
                                                RuntimeErrorKind::IntegerOverflow,
                                                "integer negation overflowed",
                                                function,
                                                pc,
                                            )
                                        })?)
                                    }
                                    DecodedValue::Float(value) => DecodedValue::Float(-value),
                                    _ => {
                                        return Err(runtime_type_error(
                                            "numeric value",
                                            &input,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    }
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(value, instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::Not { dst, src } => {
                                let input = *read_register(&registers, *src, function, pc)?;
                                let value = match input.value() {
                                    DecodedValue::Int(value) => DecodedValue::Int(!value),
                                    DecodedValue::BuiltinAtom(BuiltinAtom::True) => {
                                        DecodedValue::BuiltinAtom(BuiltinAtom::False)
                                    }
                                    DecodedValue::BuiltinAtom(BuiltinAtom::False) => {
                                        DecodedValue::BuiltinAtom(BuiltinAtom::True)
                                    }
                                    _ => {
                                        return Err(runtime_type_error(
                                            "Int or Bool",
                                            &input,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    }
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(value, instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::LogicalNot { dst, src } => {
                                let input = *read_register(&registers, *src, function, pc)?;
                                let value = match input.value() {
                                    DecodedValue::BuiltinAtom(BuiltinAtom::True) => {
                                        DecodedValue::BuiltinAtom(BuiltinAtom::False)
                                    }
                                    DecodedValue::BuiltinAtom(BuiltinAtom::False) => {
                                        DecodedValue::BuiltinAtom(BuiltinAtom::True)
                                    }
                                    _ => {
                                        return Err(runtime_type_error(
                                            "Bool", &input, &view, function, pc,
                                        ));
                                    }
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(value, instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::BitNot { dst, src } => {
                                let input = *read_register(&registers, *src, function, pc)?;
                                let DecodedValue::Int(value) = input.value() else {
                                    return Err(runtime_type_error(
                                        "Int", &input, &view, function, pc,
                                    ));
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(
                                        DecodedValue::Int(!value),
                                        instruction_location(function, pc),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::BitAnd { dst, left, right }
                            | Opcode::BitOr { dst, left, right }
                            | Opcode::BitXor { dst, left, right } => {
                                let operation = match instruction {
                                    Opcode::BitAnd { .. } => BitwiseOperation::And,
                                    Opcode::BitOr { .. } => BitwiseOperation::Or,
                                    Opcode::BitXor { .. } => BitwiseOperation::Xor,
                                    _ => unreachable!(),
                                };
                                let value = bitwise_binary(
                                    read_register(&registers, *left, function, pc)?,
                                    read_register(&registers, *right, function, pc)?,
                                    operation,
                                    &view,
                                    function,
                                    pc,
                                )?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::Equal { dst, left, right } => {
                                let left = *read_register(&registers, *left, function, pc)?;
                                let right = *read_register(&registers, *right, function, pc)?;
                                propagate_data_failures(&[left, right], &view, function, pc)?;
                                let equal =
                                    view.values_equal(left, right).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(equal)
                                        .with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::NotEqual { dst, left, right } => {
                                let left = *read_register(&registers, *left, function, pc)?;
                                let right = *read_register(&registers, *right, function, pc)?;
                                propagate_data_failures(&[left, right], &view, function, pc)?;
                                let not_equal =
                                    !view.values_equal(left, right).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(not_equal)
                                        .with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::LessThan { dst, left, right } => {
                                let left = read_register(&registers, *left, function, pc)?;
                                let right = read_register(&registers, *right, function, pc)?;
                                let less =
                                    ordered_comparison(left, right, false, &view, function, pc)?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(less).with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::LessThanOrEqual { dst, left, right } => {
                                let left = read_register(&registers, *left, function, pc)?;
                                let right = read_register(&registers, *right, function, pc)?;
                                let less_or_equal =
                                    ordered_comparison(left, right, true, &view, function, pc)?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(less_or_equal)
                                        .with_loc(instruction_location(function, pc)),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::MakeArray { dst, items } => {
                                let values = read_many(&registers, items, function, pc)?;
                                let bytes =
                                    logical_value_bytes(values.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(
                                        DecodedValue::Array(
                                            current.allocate(crate::heap::Object::Array(
                                                values.into(),
                                            )),
                                        ),
                                        instruction_location(function, pc),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::ConcatArrays { dst, arrays }
                            | Opcode::ConcatTuples { dst, tuples: arrays } => {
                                let is_tuple = matches!(instruction, Opcode::ConcatTuples { .. });
                                let arrays = read_many(&registers, arrays, function, pc)?;
                                let mut values = Vec::new();
                                for array in arrays {
                                    let handle = match (is_tuple, array.value()) {
                                        (false, DecodedValue::Array(handle))
                                        | (true, DecodedValue::Tuple(handle)) => handle,
                                        _ => return Err(runtime_type_error(
                                            if is_tuple { "Tuple spread operand" } else { "Array spread operand" },
                                            &array,
                                            &view,
                                            function,
                                            pc,
                                        )),
                                    };
                                    values.extend_from_slice(
                                        view.sequence(handle, is_tuple).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?,
                                    );
                                }
                                let bytes =
                                    logical_value_bytes(values.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                let value = if is_tuple {
                                    DecodedValue::Tuple(current.allocate(crate::heap::Object::Tuple(values.into())))
                                } else {
                                    DecodedValue::Array(current.allocate(crate::heap::Object::Array(values.into())))
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(
                                        value,
                                        instruction_location(function, pc),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::MakeTuple { dst, items } => {
                                let values = read_many(&registers, items, function, pc)?;
                                let bytes =
                                    logical_value_bytes(values.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                write_register(
                                    &mut registers,
                                    *dst,
                                    Val::new(
                                        DecodedValue::Tuple(
                                            current.allocate(crate::heap::Object::Tuple(
                                                values.into(),
                                            )),
                                        ),
                                        instruction_location(function, pc),
                                    ),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::InterpolateString { dst, parts } => {
                                let values = read_many(&registers, parts, function, pc)?
                                    .into_iter()
                                    .map(|value| {
                                        view.unwrap_declared(value).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })
                                    })
                                    .collect::<Result<Vec<_>, _>>()?;
                                let output_len = values.iter().try_fold(0usize, |length, value| {
                                    let part = crate::fmt::interpolation_value_len(
                                        crate::ValueRef {
                                            value: *value,
                                            view,
                                        },
                                    )
                                    .map_err(|native_error| {
                                        error(
                                            match native_error.limit() {
                                                Some(NativeLimit::Stack) => {
                                                    RuntimeErrorKind::StackLimitExceeded
                                                }
                                                Some(NativeLimit::Allocation) => {
                                                    RuntimeErrorKind::AllocationQuotaExceeded
                                                }
                                                None => RuntimeErrorKind::TypeMismatch,
                                            },
                                            native_error.message,
                                            function,
                                            pc,
                                        )
                                    })?;
                                    length.checked_add(part).ok_or_else(|| {
                                        allocation_error(
                                            "String allocation size overflowed",
                                            function,
                                            pc,
                                        )
                                    })
                                })?;
                                let bytes = u64::try_from(output_len).map_err(|_| {
                                    allocation_error(
                                        "String allocation size overflowed",
                                        function,
                                        pc,
                                    )
                                })?;
                                charge_allocation(account, bytes, function, pc)?;
                                let mut output = crate::fmt::string_with_capacity(output_len)
                                    .map_err(|native_error| {
                                        error(
                                            RuntimeErrorKind::AllocationQuotaExceeded,
                                            native_error.message,
                                            function,
                                            pc,
                                        )
                                    })?;
                                for value in &values {
                                    crate::fmt::write_interpolation_value(
                                        crate::ValueRef {
                                            value: *value,
                                            view,
                                        },
                                        &mut output,
                                    )
                                    .map_err(|native_error| {
                                        error(
                                            match native_error.limit() {
                                                Some(NativeLimit::Stack) => {
                                                    RuntimeErrorKind::StackLimitExceeded
                                                }
                                                Some(NativeLimit::Allocation) => {
                                                    RuntimeErrorKind::AllocationQuotaExceeded
                                                }
                                                None => RuntimeErrorKind::TypeMismatch,
                                            },
                                            native_error.message,
                                            function,
                                            pc,
                                        )
                                    })?;
                                }
                                debug_assert_eq!(output.len(), output_len);
                                let value = Val::new(
                                    current.string(Some(background), &output),
                                    instruction_location(function, pc),
                                );
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::MakeDict { dst, fields } => {
                                let (_, _, text_links, _) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let mut entries = fields
                                    .iter()
                                    .map(|(field, register)| {
                                        let field =
                                            text_links.get(field.0).copied().ok_or_else(|| {
                                                error(
                                                    RuntimeErrorKind::InvalidBytecode,
                                                    format!(
                                                        "text link {} is out of bounds",
                                                        field.0
                                                    ),
                                                    function,
                                                    pc,
                                                )
                                            })?;
                                        Ok((
                                            field,
                                            *read_register(&registers, *register, function, pc)?,
                                        ))
                                    })
                                    .collect::<Result<Vec<_>, RuntimeError>>()?;
                                entries.sort_by(|left, right| {
                                    view.text(left.0)
                                        .unwrap_or("")
                                        .cmp(view.text(right.0).unwrap_or(""))
                                });
                                if entries.windows(2).any(|pair| {
                                    view.text(pair[0].0).ok() == view.text(pair[1].0).ok()
                                }) {
                                    return Err(error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        "Dict contains a duplicate field",
                                        function,
                                        pc,
                                    ));
                                }
                                let (fields, values): (Vec<_>, Vec<_>) =
                                    entries.into_iter().unzip();
                                let field_bytes =
                                    fields.iter().try_fold(0u64, |total, field| {
                                        let length = view
                                            .text(*field)
                                            .map_err(|heap_error| {
                                                error(
                                                    RuntimeErrorKind::InvalidBytecode,
                                                    heap_error.to_string(),
                                                    function,
                                                    pc,
                                                )
                                            })?
                                            .len();
                                        total.checked_add(length as u64).ok_or_else(|| {
                                            allocation_error(
                                                "Dict allocation size overflowed",
                                                function,
                                                pc,
                                            )
                                        })
                                    })?;
                                let value_bytes =
                                    logical_value_bytes(values.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                let bytes =
                                    field_bytes.checked_add(value_bytes).ok_or_else(|| {
                                        allocation_error(
                                            "Dict allocation size overflowed",
                                            function,
                                            pc,
                                        )
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                let shape = current.intern_shape(fields);
                                let dict = Val::new(
                                    DecodedValue::Dict(current.allocate(
                                        crate::heap::Object::Dict {
                                            shape,
                                            values: values.into(),
                                        },
                                    )),
                                    instruction_location(function, pc),
                                );
                                write_register(&mut registers, *dst, dict, function, pc)?;
                            }
                            Opcode::MergeDicts { dst, .. } | Opcode::StructUpdate { dst, .. } => {
                                let (dicts, owner) = match instruction {
                                    Opcode::MergeDicts { dicts, .. } => {
                                        (read_many(&registers, dicts, function, pc)?, None)
                                    }
                                    Opcode::StructUpdate { left, right, .. } => {
                                        let base = *read_register(&registers, *left, function, pc)?;
                                        let patch = *read_register(&registers, *right, function, pc)?;
                                        (vec![base, patch], base.type_id())
                                    }
                                    _ => unreachable!(),
                                };
                                let mut merged = BTreeMap::new();
                                for dict in dicts {
                                    let DecodedValue::Dict(handle) = dict.value() else {
                                        return Err(runtime_type_error(
                                            "Dict spread operand",
                                            &dict,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    };
                                    let (fields, values) =
                                        view.dict_parts(handle).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?;
                                    for (field, value) in fields.iter().zip(values) {
                                        let field = view.text(*field).map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?;
                                        merged.insert(field.to_owned(), *value);
                                    }
                                }
                                let field_bytes =
                                    merged.keys().try_fold(0u64, |total, field| {
                                        total.checked_add(field.len() as u64).ok_or_else(|| {
                                            allocation_error(
                                                "Dict allocation size overflowed",
                                                function,
                                                pc,
                                            )
                                        })
                                    })?;
                                let value_bytes =
                                    logical_value_bytes(merged.len()).map_err(|native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    })?;
                                let bytes =
                                    field_bytes.checked_add(value_bytes).ok_or_else(|| {
                                        allocation_error(
                                            "Dict allocation size overflowed",
                                            function,
                                            pc,
                                        )
                                    })?;
                                charge_allocation(account, bytes, function, pc)?;
                                let (fields, values): (Vec<_>, Vec<_>) = merged
                                    .into_iter()
                                    .map(|(field, value)| (current.intern(&field), value))
                                    .unzip();
                                let shape = current.intern_shape(fields);
                                let dict = Val::new(
                                    DecodedValue::Dict(current.allocate(
                                        crate::heap::Object::Dict {
                                            shape,
                                            values: values.into(),
                                        },
                                    )),
                                    instruction_location(function, pc),
                                );
                                let dict = owner.map_or(dict, |owner| dict.with_type_id(owner.unchecked()));
                                write_register(&mut registers, *dst, dict, function, pc)?;
                            }
                            Opcode::GetField { dst, dict, field } => {
                                let (_, _, text_links, _) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let field = text_links.get(field.0).copied().ok_or_else(|| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        format!("text link {} is out of bounds", field.0),
                                        function,
                                        pc,
                                    )
                                })?;
                                let dict = read_register(&registers, *dict, function, pc)?;
                                let dict = view.unwrap_declared(*dict).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let value = match dict.value() {
                                    DecodedValue::Dict(handle) => view.dict_get(handle, field),
                                    DecodedValue::Module(handle) => view.exports_get(handle, field),
                                    _ => {
                                        return Err(runtime_type_error(
                                            "Dict or Module",
                                            &dict,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    }
                                }
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
                                        RuntimeErrorKind::MissingField,
                                        format!(
                                            "value has no field {:?}",
                                            view.text(field).unwrap_or("<invalid>")
                                        ),
                                        function,
                                        pc,
                                    )
                                })?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::GetArray { dst, array, index } => {
                                let array = *read_register(&registers, *array, function, pc)?;
                                let DecodedValue::Array(handle) = array.value() else {
                                    return Err(runtime_type_error(
                                        "Array", &array, &view, function, pc,
                                    ));
                                };
                                let index = *read_register(&registers, *index, function, pc)?;
                                let DecodedValue::Int(index_value) = index.value() else {
                                    return Err(runtime_type_error(
                                        "Int", &index, &view, function, pc,
                                    ));
                                };
                                let items = view.sequence(handle, false).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let value = usize::try_from(index_value)
                                    .ok()
                                    .and_then(|index| items.get(index).copied());
                                let Some(value) = value else {
                                    return Err(out_of_range_error(account, function, pc));
                                };
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::ProjectTuple { dst, tuple, index } => {
                                let tuple = *read_register(&registers, *tuple, function, pc)?;
                                let DecodedValue::Tuple(handle) = tuple.value() else {
                                    return Err(runtime_type_error(
                                        "Tuple", &tuple, &view, function, pc,
                                    ));
                                };
                                let value = view
                                    .sequence(handle, true)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .get(*index)
                                    .copied();
                                let Some(value) = value else {
                                    return Err(out_of_range_error(account, function, pc));
                                };
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::FieldExists { dst, value, field } => {
                                let (_, _, text_links, _) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let field = text_links.get(field.0).copied().ok_or_else(|| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        format!("text link {} is out of bounds", field.0),
                                        function,
                                        pc,
                                    )
                                })?;
                                let value = read_register(&registers, *value, function, pc)?;
                                propagate_direct_failure(value, function, pc)?;
                                let value = view.unwrap_declared(*value).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let exists = match value.value() {
                                    DecodedValue::Dict(handle) => view
                                        .dict_get(handle, field)
                                        .map_err(|heap_error| {
                                            error(
                                                RuntimeErrorKind::InvalidBytecode,
                                                heap_error.to_string(),
                                                function,
                                                pc,
                                            )
                                        })?
                                        .is_some(),
                                    _ => false,
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(exists),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::IsDict { dst, value } => {
                                let value = read_register(&registers, *value, function, pc)?;
                                propagate_direct_failure(value, function, pc)?;
                                let value = view.unwrap_declared(*value).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let matches = matches!(value.value(), DecodedValue::Dict(_));
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(matches),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::TupleLengthEquals { dst, value, length } => {
                                let value = read_register(&registers, *value, function, pc)?;
                                propagate_direct_failure(value, function, pc)?;
                                let value = view.unwrap_declared(*value).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let matches = matches!(
                                    value.value(),
                                    DecodedValue::Tuple(handle) if view.sequence(handle, true).is_ok_and(|items| items.len() == *length)
                                );
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(matches),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::GetTuple { dst, tuple, index } => {
                                let tuple = read_register(&registers, *tuple, function, pc)?;
                                let DecodedValue::Tuple(handle) = tuple.value() else {
                                    return Err(runtime_type_error(
                                        "Tuple", tuple, &view, function, pc,
                                    ));
                                };
                                let value = view
                                    .sequence(handle, true)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .get(*index)
                                    .copied()
                                    .ok_or_else(|| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            format!("tuple index {index} is out of bounds"),
                                            function,
                                            pc,
                                        )
                                    })?;
                                write_register(&mut registers, *dst, value, function, pc)?;
                            }
                            Opcode::TaggedTagEquals { dst, value, tag } => {
                                let value = read_register(&registers, *value, function, pc)?;
                                propagate_direct_failure(value, function, pc)?;
                                let value = view.unwrap_declared(*value).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                let expected = read_register(&registers, *tag, function, pc)?;
                                let actual = match value.value() {
                                    DecodedValue::Tagged(handle) => {
                                        let (actual, _) =
                                            view.tagged(handle).map_err(|heap_error| {
                                                error(
                                                    RuntimeErrorKind::InvalidBytecode,
                                                    heap_error.to_string(),
                                                    function,
                                                    pc,
                                                )
                                            })?;
                                        Some(actual)
                                    }
                                    DecodedValue::BuiltinAtom(_)
                                    | DecodedValue::InlineAtom(_)
                                    | DecodedValue::Atom(_) => Some(value),
                                    _ => None,
                                };
                                let matches = if let Some(actual) = actual {
                                    view.values_equal(
                                        actual.without_type_id(),
                                        expected.without_type_id(),
                                    )
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                } else {
                                    false
                                };
                                write_register(
                                    &mut registers,
                                    *dst,
                                    runtime_bool(matches),
                                    function,
                                    pc,
                                )?;
                            }
                            Opcode::GetTaggedPayload { dst, value } => {
                                let tagged = read_register(&registers, *value, function, pc)?;
                                let tagged =
                                    view.unwrap_declared(*tagged).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let DecodedValue::Tagged(handle) = tagged.value() else {
                                    return Err(runtime_type_error(
                                        "Tagged", &tagged, &view, function, pc,
                                    ));
                                };
                                let (_, payload) = view.tagged(handle).map_err(|heap_error| {
                                    error(
                                        RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(),
                                        function,
                                        pc,
                                    )
                                })?;
                                write_register(&mut registers, *dst, payload, function, pc)?;
                            }
                            Opcode::MakeClosure {
                                dst,
                                prototype,
                                captures,
                            } => {
                                let (_, _, _, prototypes) =
                                    view.bytecode(frame.prototype).map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let closure_prototype =
                                    prototypes.get(prototype.0).copied().ok_or_else(|| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            format!(
                                                "prototype link {} is out of bounds",
                                                prototype.0
                                            ),
                                            function,
                                            pc,
                                        )
                                    })?;
                                let captures = read_many(&registers, captures, function, pc)?;
                                let bytes = logical_value_bytes(captures.len()).map_err(
                                    |native_error| {
                                        allocation_error(native_error.message, function, pc)
                                    },
                                )?;
                                charge_allocation(account, bytes, function, pc)?;
                                let closure = Val::new(
                                    DecodedValue::Func(current.allocate(
                                        crate::heap::Object::Closure {
                                            identity: Arc::new(()),
                                            prototype: closure_prototype,
                                            upvalues: captures.into(),
                                        },
                                    )),
                                    instruction_location(function, pc),
                                );
                                write_register(&mut registers, *dst, closure, function, pc)?;
                            }
                            Opcode::Call {
                                base: call_base,
                                argument_count,
                            } => {
                                let callee = *read_register(&registers, *call_base, function, pc)?;
                                let arguments = read_call_arguments(
                                    &registers,
                                    *call_base,
                                    *argument_count,
                                    function,
                                    pc,
                                )?;
                                frames.last_mut().expect("caller frame").pc += 1;
                                let _ = registers;
                                match drive_vm_action(
                                    VmAction::Call {
                                        callee,
                                        arguments,
                                        return_target: ReturnTarget::Register {
                                            destination: *call_base,
                                            call_site: instruction_location(function, pc),
                                        },
                                        call_function: function_arc,
                                        call_pc: pc,
                                        rule_boundary: None,
                                    },
                                    &mut frames,
                                    &mut stack,
                                    &mut current,
                                    background,
                                    account,
                                )? {
                                    DriveOutcome::Pending => continue,
                                    DriveOutcome::Root(value) => return Ok(value),
                                }
                            }
                            Opcode::TailCall {
                                base: call_base,
                                argument_count,
                            } => {
                                let callee = *read_register(&registers, *call_base, function, pc)?;
                                let arguments = read_call_arguments(
                                    &registers,
                                    *call_base,
                                    *argument_count,
                                    function,
                                    pc,
                                )?;
                                let (return_target, rule_boundary, truncate) = if matches!(frames.last().expect("tail caller").return_target, ReturnTarget::Native(_)) {
                                    // Native continuations include diagnostic scopes and
                                    // lazy-task completion. Keep their failure boundary
                                    // while native dispatch can still fail synchronously.
                                    // The callee receives a Register target, so subsequent
                                    // tail recursion replaces frames normally.
                                    let frame = frames.last_mut().expect("native boundary");
                                    frame.tail_return = Some(*call_base);
                                    (ReturnTarget::Register { destination: *call_base, call_site: instruction_location(function, pc) }, frame.rule_boundary, None)
                                } else {
                                    let completed = frames.pop().expect("tail caller frame");
                                    (completed.return_target, completed.rule_boundary, Some(completed.base))
                                };
                                let rule_boundary = rule_boundary.or_else(|| instruction_location(function, pc));
                                let _ = registers;
                                if let Some(base) = truncate { stack.truncate(base); }
                                match drive_vm_action(
                                    VmAction::Call {
                                        callee,
                                        arguments,
                                        return_target,
                                        call_function: function_arc,
                                        call_pc: pc,
                                        rule_boundary,
                                    },
                                    &mut frames,
                                    &mut stack,
                                    &mut current,
                                    background,
                                    account,
                                )? {
                                    DriveOutcome::Pending => continue,
                                    DriveOutcome::Root(value) => return Ok(value),
                                }
                            }
                            Opcode::Jump { target } => {
                                validate_jump(*target, function, pc)?;
                                if *target <= pc {
                                    consume_fuel(account, function, pc)?;
                                }
                                frames.last_mut().expect("execution frame").pc = *target;
                                continue;
                            }
                            Opcode::JumpIfFalse { condition, target } => {
                                let condition =
                                    read_register(&registers, *condition, function, pc)?;
                                match condition.value() {
                                    DecodedValue::BuiltinAtom(BuiltinAtom::True) => {}
                                    DecodedValue::BuiltinAtom(BuiltinAtom::False) => {
                                        validate_jump(*target, function, pc)?;
                                        if *target <= pc {
                                            consume_fuel(account, function, pc)?;
                                        }
                                        frames.last_mut().expect("execution frame").pc = *target;
                                        continue;
                                    }
                                    _ => {
                                        return Err(runtime_type_error(
                                            "True or False",
                                            condition,
                                            &view,
                                            function,
                                            pc,
                                        ));
                                    }
                                }
                            }
                            Opcode::Return { src } => {
                                let value = *read_register(&registers, *src, function, pc)?;
                                let completed = frames.pop().expect("execution frame");
                                let _ = registers;
                                stack.truncate(completed.base);
                                match drive_vm_action(
                                    VmAction::Return {
                                        value,
                                        return_target: completed.return_target,
                                    },
                                    &mut frames,
                                    &mut stack,
                                    &mut current,
                                    background,
                                    account,
                                )? {
                                    DriveOutcome::Pending => continue,
                                    DriveOutcome::Root(value) => return Ok(value),
                                }
                            }
                            Opcode::Fail { message } => {
                                return Err(error(
                                    RuntimeErrorKind::NoPatternMatched,
                                    message,
                                    function,
                                    pc,
                                ));
                            }
                            Opcode::Panic { message } => {
                                let message = *read_register(&registers, *message, function, pc)?;
                                let text = view
                                    .string_text(message)
                                    .map_err(|heap_error| {
                                        error(
                                            RuntimeErrorKind::InvalidBytecode,
                                            heap_error.to_string(),
                                            function,
                                            pc,
                                        )
                                    })?
                                    .ok_or_else(|| {
                                        runtime_type_error("String", &message, &view, function, pc)
                                    })?
                                    .as_str()
                                    .to_owned();
                                return Err(error(RuntimeErrorKind::Panic, text, function, pc));
                            }
                            Opcode::Raise { action, dst, message, subjects } => {
                                let message = *read_register(&registers, *message, function, pc)?;
                                let mut values = subjects.iter().map(|subject|
                                    read_register(&registers, *subject, function, pc).copied()
                                ).collect::<Result<Vec<_>, _>>()?;
                                propagate_data_failures(&[message], &view, function, pc)?;
                                propagate_data_failures(&values, &view, function, pc)?;
                                let text = if matches!(action, crate::ast::BlameAction::Raise | crate::ast::BlameAction::Warn) {
                                    if let Some(text) = view.string_text(message)
                                        .map_err(|err| error(RuntimeErrorKind::InvalidBytecode, err.to_string(), function, pc))? {
                                        text.to_string()
                                    } else {
                                        let DecodedValue::Opaque(handle) = message.value() else {
                                            return Err(runtime_type_error("String or BlameError", &message, &view, function, pc));
                                        };
                                        let Object::Opaque(error_value) = view.object(handle).map_err(|err|
                                            error(RuntimeErrorKind::InvalidBytecode, err.to_string(), function, pc))? else {
                                            return Err(runtime_type_error("String or BlameError", &message, &view, function, pc));
                                        };
                                        let text = error_value.downcast_ref::<String>(&crate::core::blame_native_type())
                                            .ok_or_else(|| runtime_type_error("String or BlameError", &message, &view, function, pc))?.clone();
                                        values = error_value.traced.to_vec();
                                        text
                                    }
                                } else { view.string_text(message)
                                    .map_err(|heap_error| error(RuntimeErrorKind::InvalidBytecode,
                                        heap_error.to_string(), function, pc))?
                                    .ok_or_else(|| runtime_type_error("String", &message, &view, function, pc))?.to_string() };
                                // Keep diagnostic allocation accounting independent of a public value type.
                                let bytes = logical_value_bytes(values.len().saturating_add(3))
                                    .and_then(|bytes| bytes.checked_add(15).ok_or_else(||
                                        NativeError::allocation_limit("diagnostic size overflowed")))
                                    .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
                                charge_allocation(account, bytes, function, pc)?;
                                if *action == crate::ast::BlameAction::Build {
                                    charge_allocation(account, text.len() as u64, function, pc)?;
                                    let mut opaque = crate::OpaqueValue::new_identity(crate::core::blame_native_type(), text);
                                    opaque.traced = values.into_boxed_slice();
                                    let value = Val::new(DecodedValue::Opaque(current.allocate(Object::Opaque(opaque))), instruction_location(function, pc));
                                    write_register(&mut registers, *dst, value, function, pc)?;
                                } else {
                                    let location = instruction_location(function, pc);
                                    let mut runtime = error(RuntimeErrorKind::RaisedBlame, text, function, pc);
                                    runtime.set_contextual_locations(
                                        values.iter().filter_map(|value| value.loc()),
                                        location,
                                        location,
                                    );
                                    if *action == crate::ast::BlameAction::Warn {
                                        let mut diagnostic = runtime.diagnostic().unwrap_or_else(|| Diagnostic {
                                            severity: crate::source::Severity::Warning,
                                            message: runtime.message.clone(),
                                            labels: Vec::new(),
                                            notes: Vec::new(),
                                        });
                                        diagnostic.severity = crate::source::Severity::Warning;
                                        account.diagnostics.push(diagnostic);
                                        write_register(&mut registers, *dst,
                                            Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), location), function, pc)?;
                                    } else {
                                        return Err(runtime);
                                    }
                                }
                            }
                            Opcode::Debug {
                                value,
                                module,
                                line,
                                name,
                                message,
                            } => {
                                let value = *read_register(&registers, *value, function, pc)?;
                                if let Ok(value_text) = DebugValueFormatter::new(view).format(value)
                                {
                                    debug_sink.emit(DebugEvent {
                                        name: name.clone(),
                                        repr: value_text,
                                        module: module.clone(),
                                        line: *line,
                                        message: message.clone(),
                                    });
                                }
                            }
                        }
                        frames.last_mut().expect("execution frame").pc += 1;
                    }
                })();
                match attempt {
                    Err(mut runtime_error)
                        if runtime_error.failure_class()
                            == crate::evaluation::FailureClass::Recoverable
                            && frames.iter().any(|frame| {
                                matches!(
                                    &frame.return_target,
                                    ReturnTarget::Native(continuation)
                                        if continuation.catches_recoverable()
                                )
                            }) =>
                    {
                        append_runtime_trace(&mut runtime_error, &frames);
                        let frame_index = frames
                            .iter()
                            .rposition(|frame| {
                                matches!(
                                    &frame.return_target,
                                    ReturnTarget::Native(continuation)
                                        if continuation.catches_recoverable()
                                )
                            })
                            .expect("recoverable diagnostic scope exists");
                        let stack_base = frames[frame_index].base;
                        let completed = frames.drain(frame_index..).next().expect("caught frame");
                        stack.truncate(stack_base);
                        let ReturnTarget::Native(continuation) = completed.return_target else {
                            unreachable!("diagnostic scope is a native continuation")
                        };
                        let action = continuation.catch_recoverable(
                            runtime_error,
                            &mut current,
                            background,
                            account,
                        )?;
                        match drive_vm_action(
                            action,
                            &mut frames,
                            &mut stack,
                            &mut current,
                            background,
                            account,
                        )? {
                            DriveOutcome::Pending => continue,
                            DriveOutcome::Root(root) => break Ok(root),
                        }
                    }
                    Err(mut runtime_error)
                        if best_effort
                            && runtime_error.failure_class()
                                == crate::evaluation::FailureClass::Recoverable =>
                    {
                        let failure_location = runtime_error.data_location();
                        let failed_instruction = runtime_error.instruction;
                        let frame_index = frames.iter().rposition(|frame| {
                            matches!(
                                frame.return_target,
                                ReturnTarget::Native(_) | ReturnTarget::Register { .. }
                            )
                        });
                        let current_destination = if frame_index.is_none() {
                            frames.last().and_then(|frame| {
                                (frame.function.name() == runtime_error.function)
                                    .then(|| {
                                        frame
                                            .function
                                            .instructions()
                                            .get(runtime_error.instruction)
                                            .and_then(recoverable_instruction_destination)
                                    })
                                    .flatten()
                            })
                        } else {
                            None
                        };
                        if frame_index.is_none() && current_destination.is_none() {
                            break Err(runtime_error);
                        }
                        let failure_id = if let Some(failure_id) = runtime_error.propagated_failure
                        {
                            if failure_id as usize
                                >= inherited_failure_count.saturating_add(failures.len())
                            {
                                break Err(error(
                                    RuntimeErrorKind::InvalidBytecode,
                                    "failed evaluation node references an unknown root",
                                    function,
                                    0,
                                ));
                            }
                            failure_id
                        } else {
                            append_runtime_trace(&mut runtime_error, &frames);
                            let failure_id = u32::try_from(
                                inherited_failure_count.saturating_add(failures.len()),
                            )
                            .map_err(|_| {
                                error(
                                    RuntimeErrorKind::AllocationQuotaExceeded,
                                    "best-effort failure arena is full",
                                    function,
                                    0,
                                )
                            })?;
                            failures.push(runtime_error);
                            failure_id
                        };
                        let failure = Val::new(DecodedValue::Failed(failure_id), failure_location);
                        if let Some(frame_index) = frame_index {
                            let stack_base = frames[frame_index].base;
                            let completed =
                                frames.drain(frame_index..).next().expect("failed frame");
                            stack.truncate(stack_base);
                            match completed.return_target {
                                ReturnTarget::Register {
                                    destination,
                                    call_site,
                                } => {
                                    let caller =
                                        frames.last().expect("register return has a caller");
                                    let end = caller.base + caller.function.register_count();
                                    write_register(
                                        &mut stack[caller.base..end],
                                        destination,
                                        failure.rebase_generated(call_site),
                                        &caller.function,
                                        caller.pc.saturating_sub(1),
                                    )?;
                                    continue;
                                }
                                ReturnTarget::Native(continuation) => {
                                    let action = continuation.resume_failed(
                                        failure,
                                        &mut current,
                                        background,
                                        account,
                                    )?;
                                    match drive_vm_action(
                                        action,
                                        &mut frames,
                                        &mut stack,
                                        &mut current,
                                        background,
                                        account,
                                    )? {
                                        DriveOutcome::Pending => continue,
                                        DriveOutcome::Root(root) => break Ok(root),
                                    }
                                }
                                ReturnTarget::Root => unreachable!("root frame is not recoverable"),
                            }
                        } else {
                            let destination = current_destination.expect("checked above");
                            let frame = frames.last_mut().expect("execution frame");
                            let end = frame.base + frame.function.register_count();
                            write_register(
                                &mut stack[frame.base..end],
                                destination,
                                failure,
                                &frame.function,
                                failed_instruction,
                            )?;
                            frame.pc = frame.pc.max(failed_instruction.saturating_add(1));
                            continue;
                        }
                    }
                    outcome => break outcome,
                }
            }
        })();
        if publish && result.is_ok() && current.solved_evaluation.as_ref().is_some_and(|e| !e.can_publish()) {
            result = Err(if current.solved_failures.is_empty() {
                error(RuntimeErrorKind::InvalidBytecode, "demand session contains unfinished tasks", function, 0)
            } else { propagated_failure_error(0, instruction_location(function, 0), function, 0) });
        }
        if let Err(runtime_error) = &mut result {
            append_runtime_trace(runtime_error, &frames);
            if let Some(evaluation) = &mut current.solved_evaluation {
                let id = runtime_error.propagated_failure.unwrap_or_else(|| {
                    let id = current.solved_failures.len() as u32;
                    current.solved_failures.push(runtime_error.clone());
                    id
                });
                evaluation.fail_active(crate::execution_graph::FailureId(id));
            }
        }
        match result {
            Ok(root) => Ok(VmExecution {
                world: WorkWorld {
                    heap: current,
                    root,
                },
                failures,
            }),
            Err(error) => Err(VmExecutionFailure {
                heap: current,
                error,
                failures,
            }),
        }
    }

    pub(crate) fn execute_in_existing_work(
        &mut self,
        background: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        work: Heap,
        account: &mut QuotaAccount,
    ) -> Result<(Heap, Val), (Heap, RuntimeError)> {
        let diagnostic_start = account.diagnostics.len();
        let execution = self
            .execute_frame_with_policy(
                background,
                externals,
                function,
                Some(work),
                None,
                &[],
                &[],
                &[],
                account,
                false,
                0,
                true,
            )
            .map_err(|failure| (failure.heap, failure.error))?;
        let world = execution.world;
        if let Err(error) = fail_on_reported_error(account, diagnostic_start, function) {
            return Err((world.heap, error));
        }
        Ok((world.heap, world.root))
    }
}
