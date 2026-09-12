impl Vm {
    /// Initialize entry.Eval, inject declared host inputs, and invoke its
    /// precompiled adapter in a fresh WorkWorld with the same quota account.
    pub fn execute_eval_with(
        &mut self,
        entry: crate::execution_link::LinkedEntry,
        mut context: crate::EvalContext,
        limits: crate::DataLimits,
        quota: Quota,
        sources: &mut SourceDatabase,
    ) -> Result<crate::execution_link::SolvedExecution, String> {
        let timing = std::env::var_os("TELORA_DEFAULT_TIMINGS").as_deref() == Some(std::ffi::OsStr::new("1"));
        let started = timing.then(std::time::Instant::now);
        let call = entry
            .eval_call
            .ok_or("missing statically compiled entry.Eval adapter")?;
        let mut main = Heap::main();
        main.solved_types = Some(entry.types);
        main.solved_graph = Some(entry.graph);
        let mut account = QuotaAccount::new(quota).with_data_limits(limits).with_sources(sources);
        let externals = solved_module_data(&mut main, entry.data, limits, sources, &mut account)?;
        let mut world = self.initialize_linked_world(
            &mut main, &externals, &entry.bytecode, &mut account, sources,
        )?;
        let main = Arc::new(main);
        if let Some(started) = started { eprintln!("{}", serde_json::json!({"default_phase":"initialize", "elapsed_ns":started.elapsed().as_nanos()})); }
        let started = timing.then(std::time::Instant::now);
        let root = ValueRef::work(world.root, &world.heap, &main);
        let config = root
            .dict_get("config")
            .ok_or("entry.Eval.config is missing")?;
        let declared_sources = solved_eval_names(config, "sources")?;
        let declared_envs = solved_eval_names(config, "envs")?;
        let accepts_args = match config.dict_get("args").and_then(|v| v.as_atom()) {
            Some(atom) if atom.as_str() == "True" => true,
            Some(atom) if atom.as_str() == "False" => false,
            _ => return Err("entry.Eval.config.args must be Bool".into()),
        };
        let provided = context.sources.keys().cloned().collect::<Vec<_>>();
        if declared_sources != provided {
            return Err(format!(
                "eval sources do not match entry.Eval config: declared {declared_sources:?}, provided {provided:?}"
            ));
        }
        if !accepts_args && !context.args.is_empty() {
            return Err("entry.Eval config does not accept command-line arguments".into());
        }
        let env = declared_envs
            .into_iter()
            .map(|name| {
                context
                    .env
                    .remove(&name)
                    .map(|value| (name.clone(), value))
                    .ok_or_else(|| format!("cannot read declared environment variable {name:?}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut input_values = vec![];
        for (name, source) in context.sources {
            let value = solved_data_value(
                &mut world.heap,
                Some(&main),
                call.result_type,
                source,
                limits,
                sources,
                &mut account,
            )?;
            input_values.push((name, value));
        }
        account.register_sources(sources);
        let inputs = solved_eval_record(&mut world.heap, &mut account, input_values)?;
        let mut env_values = vec![];
        for (name, text) in env {
            account
                .charge_allocation(text.len() as u64)
                .map_err(|_| "eval environment allocation quota exceeded")?;
            env_values.push((name, Val::unknown(world.heap.string(Some(&main), &text))));
        }
        let env = solved_eval_record(&mut world.heap, &mut account, env_values)?;
        let mut args = vec![];
        for text in context.args {
            account
                .charge_allocation(text.len() as u64)
                .map_err(|_| "eval arguments allocation quota exceeded")?;
            args.push(Val::unknown(world.heap.string(Some(&main), &text)));
        }
        account
            .charge_allocation(logical_value_bytes(args.len()).map_err(|e| e.message)?)
            .map_err(|_| "eval arguments allocation quota exceeded")?;
        let args = Val::unknown(DecodedValue::Array(
            world.heap.allocate(Object::Array(args.into())),
        ));
        let argument = solved_eval_record(
            &mut world.heap,
            &mut account,
            vec![
                ("sources".into(), inputs),
                ("env".into(), env),
                ("args".into(), args),
            ],
        )?;
        if let Some(started) = started { eprintln!("{}", serde_json::json!({"default_phase":"entry_input", "elapsed_ns":started.elapsed().as_nanos()})); }
        let started = timing.then(std::time::Instant::now);
        let world = self
            .execute_in_existing_world_with_runtime_args(
                &main,
                &HashMap::new(),
                &call.bytecode,
                world,
                &[argument],
                &[],
                &mut account,
            )
            .map_err(|error| error.with_sources(sources).to_string())?;
        if let Some(started) = started { eprintln!("{}", serde_json::json!({"default_phase":"execute", "elapsed_ns":started.elapsed().as_nanos()})); }
        Ok(crate::execution_link::SolvedExecution {
            world: ExecutionWorld::new(main, world),
            result_type: call.result_type,
        })
    }
}

fn solved_module_data(
    main: &mut Heap,
    data: Vec<(crate::codegen::DataLink, crate::EvalSource)>,
    limits: crate::DataLimits,
    sources: &mut SourceDatabase,
    account: &mut QuotaAccount,
) -> Result<HashMap<String, Val>, String> {
    let mut externals = HashMap::new();
    for (link, source) in data {
        let value = solved_data_value(main, None, link.ty, source, limits, sources, account)?;
        externals.insert(link.key(), value);
    }
    Ok(externals)
}

fn solved_data_value(
    heap: &mut Heap,
    background: Option<&Heap>,
    ty: crate::mir::TypeId,
    source: crate::EvalSource,
    limits: crate::DataLimits,
    sources: &mut SourceDatabase,
    account: &mut QuotaAccount,
) -> Result<Val, String> {
    let (plan, bytes) = solved_data_plan(source, limits, sources).map_err(|diagnostics| {
        diagnostics.iter().map(|d| sources.render(d)).collect::<Vec<_>>().join("\n")
    })?;
    account.charge_allocation(bytes).map_err(|_| "data source allocation quota exceeded")?;
    account.register_sources(sources);
    Ok(crate::json::materialize_data_plan(
        &plan, heap, Some(crate::json::SemanticDataTarget {
            background, type_id: crate::TypeId::solved(ty),
        }),
    ).value)
}

fn solved_data_plan(
    source: crate::EvalSource,
    limits: crate::DataLimits,
    sources: &mut SourceDatabase,
) -> Result<(crate::json::ValidatedDataPlan, u64), Vec<Diagnostic>> {
    if source.text.len() > limits.file_size {
        return Err(vec![Diagnostic {
            severity: crate::source::Severity::Error,
            message: "data source exceeds file_size limit".into(), labels: vec![], notes: vec![],
        }]);
    }
    let id = sources.add(source.source_name, &source.text);
    let error = |message| vec![Diagnostic::error(message, crate::Loc { source: id, start: 0, end: 0 })];
    let plan = match source.format {
        crate::SystemDataFormat::Json => crate::json::validate_json_registered(sources, id),
        crate::SystemDataFormat::Yaml => crate::yaml::validate_yaml_registered(sources, id),
        crate::SystemDataFormat::Toml => crate::toml::validate_toml_registered(sources, id),
    }?;
    let stats = plan
        .enforce_limits(limits, source.text.len())
        .map_err(|e| error(e.to_string()))?;
    let bytes = logical_value_bytes(stats.nodes.saturating_mul(4))
        .map_err(|e| error(e.message))?
        .saturating_add(stats.payloads_bytes as u64);
    Ok((plan, bytes))
}

fn solved_eval_names(config: ValueRef<'_>, field: &str) -> Result<Vec<String>, String> {
    let values = config
        .dict_get(field)
        .ok_or_else(|| format!("entry.Eval.config.{field} is missing"))?;
    let count = values
        .sequence_len()
        .ok_or("eval config expects Array(String)")?;
    let mut names = (0..count)
        .map(|index| {
            values
                .sequence_get(index)
                .and_then(|v| v.as_str())
                .map(|s| s.as_str().to_owned())
                .ok_or_else(|| "eval config expects Array(String)".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    if names.iter().any(String::is_empty) || names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(format!(
            "entry.Eval.config.{field} must contain unique non-empty names"
        ));
    }
    Ok(names)
}

fn solved_eval_record(
    heap: &mut Heap,
    account: &mut QuotaAccount,
    fields: Vec<(String, Val)>,
) -> Result<Val, String> {
    let bytes = logical_value_bytes(fields.len().saturating_mul(2))
        .map_err(|e| e.message)?
        .saturating_add(
            fields
                .iter()
                .map(|(name, _)| name.len() as u64)
                .sum::<u64>(),
        );
    account
        .charge_allocation(bytes)
        .map_err(|_| "eval context allocation quota exceeded")?;
    heap.record_value(fields).map_err(|e| e.to_string())
}
