impl fmt::Debug for ModuleRuntime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModuleRuntime")
            .finish_non_exhaustive()
    }
}

#[allow(clippy::too_many_arguments)]
fn invoke_world_member_in(
    main: &Heap,
    externals: &HashMap<String, Val>,
    sources: &SourceDatabase,
    world: WorkWorld,
    member: &str,
    runtime_arguments: &[Val],
    retain_module: bool,
    quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
) -> Result<WorkWorld, ModuleError> {
    let argument_count = runtime_arguments.len();
    let base = argument_count + 1;
    let mut instructions = vec![Instruction::GetField {
        dst: Register(base),
        dict: Register(0),
        field: member.to_owned(),
    }];
    instructions.extend((0..argument_count).map(|index| Instruction::Move {
        dst: Register(base + 1 + index),
        src: Register(1 + index),
    }));
    instructions.push(Instruction::Call {
        base: Register(base),
        argument_count,
    });
    let result = if retain_module {
        instructions.push(Instruction::MakeTuple {
            dst: Register(base + 1),
            items: vec![Register(0), Register(base)],
        });
        Register(base + 1)
    } else {
        Register(base)
    };
    instructions.push(Instruction::Return { src: result });
    let wrapper = BytecodeFunction::with_signature(
        format!("<invoke module export {member}>"),
        argument_count + 1,
        0,
        (base + argument_count + 2).max(result.0 + 1),
        Vec::new(),
        instructions,
    );
    let mut account = QuotaAccount::new(quota);
    Vm::new()
        .with_debug_sink(debug_sink)
        .execute_in_existing_world_with_runtime_args(
            main,
            externals,
            &wrapper,
            world,
            runtime_arguments,
            &[],
            &mut account,
        )
        .map_err(|error| ModuleError::new(error.with_sources(sources).to_string()))
}

#[allow(clippy::too_many_arguments)]
fn prepare_and_initialize_entry_in(
    main: &Heap,
    externals: &HashMap<String, Val>,
    sources: &SourceDatabase,
    world: WorkWorld,
    provider: Val,
    caps: Val,
    value_owner: Val,
    prepared_data: Val,
    initializer: Val,
    main_argument: Val,
    quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
) -> Result<WorkWorld, ModuleError> {
    let wrapper = BytecodeFunction::with_signature(
        "<prepare resources and invoke Entry initializer>",
        7,
        0,
        14,
        Vec::new(),
        vec![
            Instruction::Move {
                dst: Register(7),
                src: Register(1),
            },
            Instruction::Move {
                dst: Register(8),
                src: Register(2),
            },
            Instruction::Move {
                dst: Register(9),
                src: Register(3),
            },
            Instruction::Move {
                dst: Register(10),
                src: Register(4),
            },
            Instruction::Call {
                base: Register(7),
                argument_count: 3,
            },
            Instruction::Move {
                dst: Register(11),
                src: Register(5),
            },
            Instruction::Move {
                dst: Register(12),
                src: Register(7),
            },
            Instruction::Move {
                dst: Register(13),
                src: Register(6),
            },
            Instruction::Call {
                base: Register(11),
                argument_count: 2,
            },
            Instruction::Return { src: Register(11) },
        ],
    );
    let arguments = [
        provider,
        caps,
        value_owner,
        prepared_data,
        initializer,
        main_argument,
    ];
    let mut account = QuotaAccount::new(quota);
    Vm::new()
        .with_debug_sink(debug_sink)
        .execute_in_existing_world_with_runtime_args(
            main,
            externals,
            &wrapper,
            world,
            &arguments,
            &[],
            &mut account,
        )
        .map_err(|error| ModuleError::new(error.with_sources(sources).to_string()))
}

impl LoadedModule {
    pub const fn uses_explicit_exports(&self) -> bool {
        self.analysis.explicit_exports
    }

    pub fn execute(
        &self,
        evaluation_fuel: usize,
    ) -> Result<crate::ExecutionWorld, crate::RuntimeError> {
        self.execute_with_quota(Quota::with_fuel(evaluation_fuel))
    }

    pub fn execute_with_quota(
        &self,
        quota: Quota,
    ) -> Result<crate::ExecutionWorld, crate::RuntimeError> {
        self.execute_with_quota_and_debug_sink(quota, Arc::new(DiscardDebugSink))
    }

    pub fn execute_with_quota_and_debug_sink(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<crate::ExecutionWorld, crate::RuntimeError> {
        self.execute_observed(quota, debug_sink).0
    }

    fn check_with_quota_and_debug_sink(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<(), crate::RuntimeError> {
        self.check_observed(quota, debug_sink).0
    }

    fn check_observed(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> (Result<(), crate::RuntimeError>, Vec<Diagnostic>) {
        let mut account = QuotaAccount::new(quota);
        let result = Vm::new()
            .with_debug_sink(debug_sink)
            .execute_in_work(
                &self.runtime.main.heap,
                &self.runtime.externals,
                &self.function,
                &[],
                &mut account,
            )
            .map(|_| ())
            .map_err(|error| error.with_sources(&self.sources));
        (result, account.take_diagnostics())
    }

    fn execute_observed(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> (
        Result<crate::ExecutionWorld, crate::RuntimeError>,
        Vec<Diagnostic>,
    ) {
        let (result, diagnostics) = self.execute_raw_world_observed(quota, debug_sink);
        let result = result
            .map(|world| crate::ExecutionWorld::new(Arc::clone(&self.runtime.main.heap), world));
        (result, diagnostics)
    }

    fn execute_world_observed(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> (Result<WorkWorld, crate::RuntimeError>, Vec<Diagnostic>) {
        let (result, diagnostics) = self.execute_raw_world_observed(quota, debug_sink);
        let result = result.and_then(|world| {
            if self.analysis.explicit_exports {
                world
                    .seal_module()
                    .map_err(|error| crate::RuntimeError::from_heap_error(&self.function, error))
            } else {
                Ok(world)
            }
        });
        (result, diagnostics)
    }

    fn execute_raw_world_observed(
        &self,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> (Result<WorkWorld, crate::RuntimeError>, Vec<Diagnostic>) {
        let mut account = QuotaAccount::new(quota);
        let result = Vm::new()
            .with_debug_sink(debug_sink)
            .execute_in_work(
                &self.runtime.main.heap,
                &self.runtime.externals,
                &self.function,
                &[],
                &mut account,
            )
            .map_err(|error| error.with_sources(&self.sources));
        (result, account.take_diagnostics())
    }

    fn invoke_reducer_in_work(
        &self,
        reducer: Val,
        state: WorkWorld,
        event: Val,
        quota: Quota,
        debug_sink: Arc<dyn DebugSink>,
    ) -> Result<WorkWorld, ModuleError> {
        let wrapper = BytecodeFunction::with_signature(
            "<invoke Entry reducer>",
            3,
            0,
            6,
            Vec::new(),
            vec![
                Instruction::Move {
                    dst: Register(3),
                    src: Register(1),
                },
                Instruction::Move {
                    dst: Register(4),
                    src: Register(0),
                },
                Instruction::Move {
                    dst: Register(5),
                    src: Register(2),
                },
                Instruction::Call {
                    base: Register(3),
                    argument_count: 2,
                },
                Instruction::Return { src: Register(3) },
            ],
        );
        let mut account = QuotaAccount::new(quota);
        let result = Vm::new()
            .with_debug_sink(debug_sink)
            .execute_in_existing_world_with_runtime_args(
                &self.runtime.main.heap,
                &self.runtime.externals,
                &wrapper,
                state,
                &[reducer, event],
                &[],
                &mut account,
            );
        result.map_err(|error| ModuleError::new(error.with_sources(&self.sources).to_string()))
    }
}
