impl Vm {
    fn initialize_linked_world(
        &mut self,
        main: &mut Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        account: &mut QuotaAccount,
        sources: &SourceDatabase,
    ) -> Result<WorkWorld, String> {
        let diagnostic_start = account.diagnostics.len();
        let success = self.execute_frame_with_policy(
            main, externals, function, None, None, &[], &[], &[], account,
            false, 0, true,
        ).map_err(|failure| {
            let root = failure.error.propagated_failure
                .and_then(|id| failure.heap.solved_failures.get(id as usize))
                .unwrap_or(&failure.error);
            root.clone().with_sources(sources).to_string()
        })?;
        fail_on_reported_error(account, diagnostic_start, function)
            .map_err(|error| error.with_sources(sources).to_string())?;
        freeze_initialized_world(main, success.world)
    }

    /// Check one sealed session. Only diagnostics leave the VM; initialized
    /// globals and property objects are committed together to MainWorld.
    pub fn check_linked(
        &mut self,
        entry: crate::execution_link::LinkedEntry,
        quota: Quota,
        limits: crate::DataLimits,
        sources: &mut SourceDatabase,
    ) -> Vec<Diagnostic> {
        let diagnostic = |message: String| Diagnostic {
            severity: crate::source::Severity::Error,
            message,
            labels: vec![],
            notes: vec![],
        };
        if entry.root != crate::codegen::CompilationRoot::Check {
            return vec![diagnostic(
                "check requires a session initialization root".into(),
            )];
        }
        let mut main = Heap::main();
        main.solved_types = Some(entry.types);
        main.solved_graph = Some(entry.graph);
        let mut account = QuotaAccount::new(quota).with_data_limits(limits).with_sources(sources);
        let externals =
            match solved_module_data(&mut main, entry.data, limits, sources, &mut account) {
                Ok(externals) => externals,
                Err(error) => return vec![diagnostic(error)],
            };
        let result = self.execute_frame_with_policy(
            &main,
            &externals,
            &entry.bytecode,
            None,
            None,
            &[],
            &[],
            &[],
            &mut account,
            false,
            0,
            true,
        );
        let mut diagnostics = account.take_diagnostics();
        if let Ok(success) = result {
            if let Err(error) = freeze_initialized_world(&mut main, success.world) {
                diagnostics.push(diagnostic(error));
            }
        } else if let Err(failure) = result {
            let root = failure
                .error
                .propagated_failure
                .and_then(|id| failure.heap.solved_failures.get(id as usize))
                .unwrap_or(&failure.error);
            let error = root
                .diagnostic()
                .unwrap_or_else(|| diagnostic(root.to_string()));
            if !diagnostics.contains(&error) {
                diagnostics.push(error);
            }
        }
        diagnostics
    }
}

/// Commit initialization as one session operation. All required tasks must have
/// completed; stable graph IDs index the copied values in MainWorld afterwards.
fn freeze_initialized_world(main: &mut Heap, world: WorkWorld) -> Result<WorkWorld, String> {
    let evaluation = world.heap.solved_evaluation.as_ref().ok_or("initialization has no evaluation table")?;
    let graph = main.solved_graph.as_ref().ok_or("initialization has no execution graph")?;
    if main.solved_evaluation.is_some() { return Err("MainWorld is already initialized".into()); }
    if !evaluation.can_publish() { return Err("initialization contains failed or active tasks".into()); }
    for &node in graph.initializers() {
        if evaluation.ready(node).is_none() {
            return Err(format!("initialization is incomplete: {}", graph.nodes()[node.index()].label));
        }
    }
    let mut roots = vec![world.root];
    roots.extend_from_slice(evaluation.values());
    let mut copied = crate::heap::publish_initialized_roots(main, &world.heap, &roots)
        .map_err(|error| error.to_string())?.into_iter();
    let root = copied.next().expect("initialization root");
    main.solved_failures = world.heap.solved_failures;
    main.solved_evaluation = Some(world.heap.solved_evaluation.expect("validated evaluation table").with_values(copied.collect()));
    Ok(WorkWorld { heap: Heap::work(), root })
}
