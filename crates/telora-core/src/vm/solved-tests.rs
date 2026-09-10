impl Vm {
    /// Execute direct deferred tests in one VM-owned heap. Only the report leaves
    /// the session. Fixture expansion will use this same session boundary.
    pub fn test_linked(
        &mut self,
        entry: crate::execution_link::LinkedEntry,
        plan: crate::test_plan::TestPlan,
        quota: Quota,
        limits: crate::DataLimits,
        sources: &mut SourceDatabase,
    ) -> Result<crate::test_plan::TestReport, String> {
        use crate::{
            bytecode::Instruction as I,
            test_plan::{TestReport, TestResult},
            test_protocol::TestKind,
        };
        if entry.root != crate::codegen::CompilationRoot::Tests(plan.module) {
            return Err("test plan requires its compiled session bootstrap".into());
        }
        let mut main = Heap::main();
        main.solved_types = Some(entry.types);
        main.solved_graph = Some(entry.graph);
        let mut account = QuotaAccount::new(quota)
            .with_data_limits(limits)
            .with_sources(sources);
        let externals = solved_module_data(&mut main, entry.data, limits, sources, &mut account)?;
        let bootstrap = self
            .execute_frame_with_policy(
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
                false,
            )
            .map_err(|failure| failure.error.to_string())?;
        let mut current = Some(bootstrap.world.heap);
        let mut report = TestReport {
            diagnostics: account.take_diagnostics(),
            ..TestReport::default()
        };
        for case in plan.exports {
            let node = main
                .solved_graph
                .as_ref()
                .unwrap()
                .global(case.target)
                .ok_or("test has no execution task")?;
            let demand = BytecodeFunction::with_signature(
                "<test export>",
                0,
                0,
                1,
                vec![],
                vec![
                    I::Demand {
                        dst: Register(0),
                        node,
                    },
                    I::Return { src: Register(0) },
                ],
            );
            let mut result = TestResult {
                name: case.name,
                phase: "initialization",
                passed: false,
                diagnostics: vec![],
            };
            let value = match self.solved_test_call(
                &main,
                &externals,
                &demand,
                &mut current,
                &[],
                &mut account,
            ) {
                Ok(value) => value,
                Err(error) => {
                    report.aborted |=
                        error.failure_class() == crate::evaluation::FailureClass::Terminal;
                    result.diagnostics = account.take_diagnostics();
                    append_test_error(&mut result.diagnostics, &error, Some(case.location));
                    report.cases.push(result);
                    if report.aborted {
                        break;
                    }
                    continue;
                }
            };
            let (description, callable) = (ValueRef {
                value,
                view: HeapView {
                    current: current.as_ref().unwrap(),
                    background: Some(&main),
                },
            })
            .test_description()
            .map(|(description, callable)| (description.clone(), callable))
            .ok_or("compiled Test export has no runtime witness")?;
            if description.kind == TestKind::Fixtures {
                result.phase = "discovery";
                result.diagnostics.push(Diagnostic::error(
                    "fixture expansion is not connected to the solved test session yet",
                    case.location,
                ));
                report.cases.push(result);
                report.aborted = true;
                break;
            }
            result.diagnostics = account.take_diagnostics();
            result.phase = "execution";
            let call = BytecodeFunction::with_signature(
                "<test thunk>",
                1,
                0,
                1,
                vec![],
                vec![
                    I::Call {
                        base: Register(0),
                        argument_count: 0,
                    },
                    I::Return { src: Register(0) },
                ],
            );
            let execution = self.solved_test_call(
                &main,
                &externals,
                &call,
                &mut current,
                &[callable],
                &mut account,
            );
            let terminal = execution.as_ref().err().is_some_and(|error| {
                error.failure_class() == crate::evaluation::FailureClass::Terminal
            });
            result.passed = !terminal
                && match description.kind {
                    TestKind::ShouldOk => execution.is_ok(),
                    TestKind::ShouldFail => execution.is_err(),
                    TestKind::ShouldFailWith => execution.as_ref().err().is_some_and(|error| {
                        error
                            .message
                            .contains(description.expected.as_deref().unwrap_or(""))
                    }),
                    TestKind::Fixtures => unreachable!(),
                };
            result
                .diagnostics
                .extend(account.take_diagnostics().into_iter().filter(|d| {
                    !result.passed
                        || description.kind == TestKind::ShouldOk
                        || d.severity != crate::source::Severity::Error
                }));
            if !result.passed {
                match execution {
                    Err(error) => {
                        append_test_error(&mut result.diagnostics, &error, description.origin)
                    }
                    Ok(_) => {}
                }
                if !terminal && description.kind != TestKind::ShouldOk {
                    let message = description
                        .expected
                        .map(|expected| {
                            format!("expected a recoverable failure containing {expected:?}")
                        })
                        .unwrap_or_else(|| {
                            "expected a recoverable failure, but thunk returned normally".into()
                        });
                    result
                        .diagnostics
                        .push(Diagnostic::error(message, case.location));
                }
            }
            report.aborted |= terminal;
            report.cases.push(result);
            if report.aborted {
                break;
            }
        }
        Ok(report)
    }

    fn solved_test_call(
        &mut self,
        main: &Heap,
        externals: &HashMap<String, Val>,
        function: &BytecodeFunction,
        current: &mut Option<Heap>,
        arguments: &[Val],
        account: &mut QuotaAccount,
    ) -> Result<Val, RuntimeError> {
        match self.execute_frame_with_policy(
            main,
            externals,
            function,
            current.take(),
            None,
            arguments,
            &[],
            &[],
            account,
            false,
            0,
            false,
        ) {
            Ok(execution) => {
                let world = execution.world;
                *current = Some(world.heap);
                Ok(world.root)
            }
            Err(failure) => {
                let error = failure
                    .error
                    .propagated_failure
                    .and_then(|id| failure.heap.solved_failures.get(id as usize))
                    .cloned()
                    .unwrap_or(failure.error);
                *current = Some(failure.heap);
                Err(error)
            }
        }
    }
}

fn append_test_error(
    diagnostics: &mut Vec<Diagnostic>,
    error: &RuntimeError,
    location: Option<crate::Loc>,
) {
    let diagnostic = error.diagnostic().unwrap_or_else(|| Diagnostic {
        severity: crate::source::Severity::Error,
        message: error.message.clone(),
        labels: location
            .map(|location| crate::source::Label {
                location,
                message: String::new(),
                primary: true,
            })
            .into_iter()
            .collect(),
        notes: vec![],
    });
    if !diagnostics.contains(&diagnostic) {
        diagnostics.push(diagnostic);
    }
}
