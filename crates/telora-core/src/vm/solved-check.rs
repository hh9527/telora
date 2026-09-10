impl Vm {
    /// Check one sealed session. Only diagnostics leave the VM; initialized
    /// globals and property objects stay in its Work world until it is dropped.
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
        if let Err(failure) = result {
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
