const EVAL_SUPPORT_SOURCE: &str = r#"
import "std/value" {Value};
export type Output = Value;
"#;

#[derive(Clone, Debug, Default)]
pub struct EvalContext {
    pub sources: BTreeMap<String, EvalSource>,
    pub env: BTreeMap<String, String>,
    pub args: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct EvalSource {
    pub source_name: String,
    pub format: SystemDataFormat,
    pub text: String,
}

struct PreparedEvalContext {
    sources: Vec<(String, ValidatedDataPlan)>,
    env: BTreeMap<String, String>,
    args: Vec<String>,
}

enum EvalKind {
    Value,
    With(EvalContext),
}

impl Engine {
    pub fn eval_pending_export(
        &self,
        pending: PendingModule,
        export: &str,
    ) -> Result<String, ModuleError> {
        self.eval_pending_inner(pending, export, EvalKind::Value)
    }

    pub fn eval_pending_export_with(
        &self,
        pending: PendingModule,
        export: &str,
        context: EvalContext,
    ) -> Result<String, ModuleError> {
        self.eval_pending_inner(pending, export, EvalKind::With(context))
    }

    fn eval_pending_inner(
        &self,
        pending: PendingModule,
        export: &str,
        kind: EvalKind,
    ) -> Result<String, ModuleError> {
        let resolver = pending.inner.resolver.clone();
        let support_id = ModuleCName::builtin("std/_eval");
        let SelectedEntryLoader {
            mut loader,
            main_module,
            main_path,
            entry: support_compiled,
        } = prepare_selected_entry(
            resolver,
            support_id,
            EVAL_SUPPORT_SOURCE,
            self.config.module_quota,
            self.config.data_limits,
            Arc::clone(&self.debug_sink),
        )?;

        let mut account = QuotaAccount::new(self.config.session_quota);
        let support_world = Vm::new()
            .with_debug_sink(Arc::clone(&self.debug_sink))
            .execute_in_work(
                &loader.main.heap,
                &support_compiled.externals,
                &support_compiled.function,
                &[],
                &mut account,
            )
            .map_err(|error| ModuleError::new(error.with_sources(&loader.sources).to_string()))?
            .seal_module()
            .map_err(|error| ModuleError::new(error.to_string()))?;

        let bindings = pending.begin_initialization()?;
        let (compiled_main_path, main_compiled) = match loader.compile_root(main_module, bindings) {
            Ok(compiled) => compiled,
            Err(error) => {
                pending.finish_initialization(&Err(error.clone()));
                return Err(error);
            }
        };
        debug_assert_eq!(compiled_main_path, main_path);
        let (value_owner, value_type) =
            match semantic_value_contract(&loader.builtin_modules, &loader.main.heap) {
                Ok(contract) => contract,
                Err(error) => {
                    pending.finish_initialization(&Err(error.clone()));
                    return Err(error);
                }
            };
        let kind = match kind {
            EvalKind::Value => None,
            EvalKind::With(context) => match prepare_eval_context(
                &mut loader.sources,
                self.config.data_limits,
                context,
            ) {
                Ok(context) => Some(context),
                Err(error) => {
                    pending.finish_initialization(&Err(error.clone()));
                    return Err(error);
                }
            },
        };
        if let Err(error) = validate_eval_export(
            &main_compiled.analysis.module_interface,
            export,
            &value_type,
            kind.is_some(),
        ) {
            pending.finish_initialization(&Err(error.clone()));
            return Err(error);
        }

        let workspace = WorkspaceSnapshot::build(
            loader.sources.clone(),
            loader.semantic_inputs.values().cloned().collect(),
        );
        let dependencies = loader.dependencies.iter().cloned().collect::<Vec<_>>();
        let sources = loader.sources.clone();
        let shared_main = Arc::new(loader.main.seal());
        let support = loaded_from_compiled(
            main_path.clone(),
            dependencies.clone(),
            sources.clone(),
            workspace.clone(),
            Arc::clone(&shared_main),
            support_compiled,
        );
        let main = loaded_from_compiled(
            compiled_main_path,
            dependencies,
            sources,
            workspace,
            Arc::clone(&shared_main),
            main_compiled,
        );
        let (main_world, _) =
            main.execute_world_observed(self.config.session_quota, Arc::clone(&self.debug_sink));
        let main_world = match main_world {
            Ok(world) => world,
            Err(error) => {
                let error = ModuleError::new(error.to_string());
                pending.finish_initialization(&Err(error.clone()));
                return Err(error);
            }
        };
        let instantiated = InstantiatedModule {
            module: Arc::new(main),
            execution: Arc::new(main_world),
        };
        pending.finish_initialization(&Ok(instantiated.clone()));

        let (mut world, main_root) = support_world
            .import_world_root(&shared_main.heap, &instantiated.execution)
            .map_err(|error| ModuleError::new(error.to_string()))?;
        world.set_root(main_root);
        let world = match kind {
            None => select_world_member_in(
                &support.runtime.main.heap,
                &support.runtime.externals,
                &support.sources,
                world,
                export,
                self.config.session_quota,
                Arc::clone(&self.debug_sink),
            )?,
            Some(context) => {
                let argument = make_eval_context(
                    &mut world,
                    &shared_main.heap,
                    value_owner.runtime(),
                    context,
                )?;
                invoke_world_member_in(
                    &support.runtime.main.heap,
                    &support.runtime.externals,
                    &support.sources,
                    world,
                    export,
                    &[argument],
                    false,
                    self.config.session_quota,
                    Arc::clone(&self.debug_sink),
                )?
            }
        };
        crate::ExecutionWorld::new(Arc::clone(&shared_main.heap), world)
            .into_semantic_json()
            .map_err(ModuleError::new)
    }
}

fn validate_eval_export(
    interface: &ModuleInterface,
    export: &str,
    value_type: &TypeDescriptor,
    with_context: bool,
) -> Result<(), ModuleError> {
    let scheme = interface
        .exports
        .get(export)
        .ok_or_else(|| ModuleError::new(format!("module has no export {export:?}")))?;
    if !scheme.parameters.is_empty() {
        return Err(ModuleError::new(format!(
            "eval export {export:?} must not be polymorphic"
        )));
    }
    let context_type = TypeDescriptor::Struct(BTreeMap::from([
        (
            "args".into(),
            TypeDescriptor::Array(Box::new(TypeDescriptor::String)),
        ),
        (
            "env".into(),
            TypeDescriptor::Dict(Box::new(TypeDescriptor::String)),
        ),
        (
            "sources".into(),
            TypeDescriptor::Dict(Box::new(value_type.clone())),
        ),
    ]));
    let expected = TypeDescriptor::Function {
        parameters: vec![context_type.clone()],
        result: Box::new(value_type.clone()),
    };
    let valid = if with_context {
        match &scheme.body {
            TypeDescriptor::Function { parameters, result } if parameters.len() == 1 => {
                let parameter = crate::types::erase_declared_identity(&parameters[0]);
                let expected_parameter = crate::types::erase_declared_identity(&context_type);
                crate::types::assignable(&parameter, &expected_parameter)
                    && crate::types::assignable(&expected_parameter, &parameter)
                    && crate::types::assignable(result, value_type)
                    && crate::types::assignable(value_type, result)
            }
            _ => false,
        }
    } else {
        crate::types::assignable(&scheme.body, value_type)
            && crate::types::assignable(value_type, &scheme.body)
    };
    if valid {
        Ok(())
    } else {
        Err(ModuleError::new(format!(
            "eval export {export:?} has type {}, expected {}",
            scheme.body.display_name(),
            if with_context {
                expected.display_name()
            } else {
                value_type.display_name()
            }
        )))
    }
}

fn make_eval_context(
    world: &mut WorkWorld,
    main: &Heap,
    value_owner: Val,
    context: PreparedEvalContext,
) -> Result<Val, ModuleError> {
    let mut sources = Vec::with_capacity(context.sources.len());
    let type_id = semantic_value_type_id(world.heap(), Some(main), value_owner)
        .map_err(|error| ModuleError::new(error.to_string()))?;
    for (name, plan) in context.sources {
        let value = materialize_data_plan(
            &plan,
            world.heap_mut(),
            Some(SemanticDataTarget {
                background: Some(main),
                type_id,
            }),
        )
        .value;
        sources.push((name, value));
    }
    let sources = allocate_record(world.heap_mut(), sources);
    let env = context
        .env
        .into_iter()
        .map(|(name, value)| (name, runtime_string(world.heap_mut(), main, &value)))
        .collect::<Vec<_>>();
    let env = allocate_record(world.heap_mut(), env);
    let args = context
        .args
        .into_iter()
        .map(|value| runtime_string(world.heap_mut(), main, &value))
        .collect::<Vec<_>>();
    let args = runtime_array(world.heap_mut(), args);
    Ok(allocate_record(
        world.heap_mut(),
        vec![("args".into(), args), ("env".into(), env), ("sources".into(), sources)],
    ))
}

fn prepare_eval_context(
    sources: &mut SourceDatabase,
    limits: DataLimits,
    context: EvalContext,
) -> Result<PreparedEvalContext, ModuleError> {
    let mut prepared = Vec::with_capacity(context.sources.len());
    for (name, source) in context.sources {
        let source_id = sources.add(source.source_name.clone(), &source.text);
        let plan = validate_system_data_source(source.format, sources, source_id).map_err(
            |diagnostics| {
                ModuleError::new(
                    diagnostics
                        .iter()
                        .map(|diagnostic| sources.render(diagnostic))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
            },
        )?;
        plan.enforce_limits(limits, source.text.len())
            .map_err(|error| ModuleError::new(format!("eval source {:?}: {error}", source.source_name)))?;
        prepared.push((name, plan));
    }
    Ok(PreparedEvalContext {
        sources: prepared,
        env: context.env,
        args: context.args,
    })
}

#[allow(clippy::too_many_arguments)]
fn select_world_member_in(
    main: &Heap,
    externals: &HashMap<String, Val>,
    sources: &SourceDatabase,
    world: WorkWorld,
    member: &str,
    quota: Quota,
    debug_sink: Arc<dyn DebugSink>,
) -> Result<WorkWorld, ModuleError> {
    let wrapper = BytecodeFunction::with_signature(
        format!("<select module export {member}>"),
        1,
        0,
        2,
        Vec::new(),
        vec![
            Instruction::GetField {
                dst: Register(1),
                dict: Register(0),
                field: member.to_owned(),
            },
            Instruction::Return { src: Register(1) },
        ],
    );
    let mut account = QuotaAccount::new(quota);
    Vm::new()
        .with_debug_sink(debug_sink)
        .execute_in_existing_world_with_runtime_args(
            main,
            externals,
            &wrapper,
            world,
            &[],
            &[],
            &mut account,
        )
        .map_err(|error| ModuleError::new(error.with_sources(sources).to_string()))
}
