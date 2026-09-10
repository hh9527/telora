impl Vm {
    /// Execute deferred tests and fixture factories in one VM-owned heap.
    /// Only the report leaves the session.
    pub fn test_linked(
        &mut self,
        entry: crate::execution_link::LinkedEntry,
        plan: crate::test_plan::TestPlan,
        quota: Quota,
        limits: crate::DataLimits,
        sources: &mut SourceDatabase,
        mut context: crate::TestContext<'_>,
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
        let mut report = TestReport::default();
        let mut externals = HashMap::new();
        for (link, source) in entry.data {
            let (data, bytes) = match solved_data_plan(source, limits, sources) {
                Ok(data) => data,
                Err(diagnostics) => {
                    report.diagnostics.extend(diagnostics);
                    report.aborted = true;
                    continue;
                }
            };
            if account.charge_allocation(bytes).is_err() {
                report.diagnostics.push(Diagnostic::error(
                    "data source allocation quota exceeded",
                    link.location,
                ));
                report.aborted = true;
                break;
            }
            let value = crate::json::materialize_data_plan(
                &data,
                &mut main,
                Some(crate::json::SemanticDataTarget {
                    background: None,
                    type_id: crate::TypeId::solved(link.ty),
                }),
            )
            .value;
            externals.insert(link.key(), value);
        }
        if report.aborted {
            return Ok(report);
        }
        account.register_sources(sources);
        let bootstrap = match self.execute_frame_with_policy(
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
        ) {
            Ok(bootstrap) => bootstrap,
            Err(failure) => {
                report.aborted = true;
                report.diagnostics = account.take_diagnostics();
                append_test_error(&mut report.diagnostics, &failure.error, None);
                return Ok(report);
            }
        };
        let mut current = Some(bootstrap.world.heap);
        let mut report = TestReport {
            diagnostics: account.take_diagnostics(),
            ..TestReport::default()
        };
        let mut pending = plan
            .exports
            .into_iter()
            .rev()
            .map(SolvedTestWork::Export)
            .collect::<Vec<_>>();
        let mut expanded = 0usize;
        let mut retained = 0usize;
        while let Some(work) = pending.pop() {
            if report.aborted {
                break;
            }
            let (value, mut result, depth, location) = match work {
                SolvedTestWork::Export(case) => {
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
                        fixtures: vec![],
                        sources: vec![],
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
                    result.diagnostics = account.take_diagnostics();
                    (value, result, 0, Some(case.location))
                }
                SolvedTestWork::Value {
                    value,
                    result,
                    depth,
                    location,
                } => (value, result, depth, location),
                SolvedTestWork::Fixture {
                    prepared,
                    callable,
                    mut result,
                    depth,
                    location,
                } => {
                    if expanded >= context.limits.cases {
                        fail_solved_test(
                            &mut report,
                            result,
                            "test expansion limit exceeded",
                            location,
                            true,
                        );
                        continue;
                    }
                    let prepared = match prepared {
                        Ok(prepared) => prepared,
                        Err(diagnostics) => {
                            expanded += 1;
                            result.diagnostics.extend(diagnostics);
                            report.cases.push(result);
                            continue;
                        }
                    };
                    result.phase = "factory";
                    if account.charge_allocation(prepared.bytes).is_err() {
                        fail_solved_test(
                            &mut report,
                            result,
                            "fixture materialization allocation quota exceeded",
                            location,
                            true,
                        );
                        continue;
                    }
                    let ty = plan
                        .fixture_type
                        .ok_or("fixture factory has no statically solved input type")?;
                    account.register_sources(sources);
                    let fixture = crate::json::materialize_data_plan(
                        &prepared.plan,
                        current.as_mut().unwrap(),
                        Some(crate::json::SemanticDataTarget {
                            background: Some(&main),
                            type_id: crate::TypeId::solved(ty),
                        }),
                    )
                    .value;
                    let factory = BytecodeFunction::with_signature(
                        "<test factory>",
                        2,
                        0,
                        2,
                        vec![],
                        vec![
                            I::Call {
                                base: Register(0),
                                argument_count: 1,
                            },
                            I::Return { src: Register(0) },
                        ],
                    );
                    let next = self.solved_test_call(
                        &main,
                        &externals,
                        &factory,
                        &mut current,
                        &[callable, fixture],
                        &mut account,
                    );
                    result.diagnostics.extend(account.take_diagnostics());
                    match next {
                        Ok(value) => {
                            record_solved_test_notice(&mut report, &mut result);
                            result.phase = "discovery";
                            pending.push(SolvedTestWork::Value {
                                value,
                                result,
                                depth: depth + 1,
                                location,
                            });
                        }
                        Err(error) => {
                            expanded += 1;
                            report.aborted |=
                                error.failure_class() == crate::evaluation::FailureClass::Terminal;
                            append_test_error(&mut result.diagnostics, &error, location);
                            report.cases.push(result);
                        }
                    }
                    continue;
                }
            };
            expanded += 1;
            if expanded > context.limits.cases || depth > context.limits.depth {
                fail_solved_test(
                    &mut report,
                    result,
                    "test expansion limit exceeded",
                    location,
                    true,
                );
                continue;
            }
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
                if description.sources.is_empty() {
                    fail_solved_test(
                        &mut report,
                        result,
                        "no fixtures",
                        description.origin,
                        false,
                    );
                    continue;
                }
                if description.sources.len() > context.limits.cases.saturating_sub(expanded) {
                    fail_solved_test(
                        &mut report,
                        result,
                        "test expansion limit exceeded",
                        description.origin,
                        true,
                    );
                    continue;
                }
                record_solved_test_notice(&mut report, &mut result);
                // Prepare every immediate input before running any user factory.
                let mut cache = HashMap::new();
                let mut children = Vec::new();
                for (index, label) in description.sources.iter().enumerate() {
                    let mut child = result.clone();
                    child.fixtures.push(index);
                    child.sources.push(label.clone());
                    child.phase = "fixture";
                    let prepared = prepare_solved_fixture(
                        &mut context,
                        &plan.module_name,
                        &description,
                        &child,
                        label,
                        limits,
                        sources,
                        &mut cache,
                        &mut retained,
                    );
                    if retained > context.limits.fixture_bytes {
                        child.diagnostics.extend(prepared.err().unwrap_or_default());
                        report.cases.push(child);
                        report.aborted = true;
                        break;
                    }
                    children.push(SolvedTestWork::Fixture {
                        prepared,
                        callable,
                        result: child,
                        depth,
                        location: description.origin,
                    });
                }
                pending.extend(children.into_iter().rev());
                continue;
            }
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
                    result.diagnostics.push(solved_test_diagnostic(
                        message,
                        description.origin.or(location),
                    ));
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

enum SolvedTestWork {
    Export(crate::test_plan::TestExport),
    Value {
        value: Val,
        result: crate::test_plan::TestResult,
        depth: usize,
        location: Option<crate::Loc>,
    },
    Fixture {
        prepared: Result<SolvedFixture, Vec<Diagnostic>>,
        callable: Val,
        result: crate::test_plan::TestResult,
        depth: usize,
        location: Option<crate::Loc>,
    },
}

struct SolvedFixture {
    plan: crate::json::ValidatedDataPlan,
    bytes: u64,
}

fn prepare_solved_fixture(
    context: &mut crate::TestContext<'_>,
    module: &str,
    description: &crate::test_protocol::TestDescription,
    case: &crate::test_plan::TestResult,
    label: &str,
    limits: crate::DataLimits,
    sources: &mut SourceDatabase,
    cache: &mut HashMap<String, Result<String, String>>,
    retained: &mut usize,
) -> Result<SolvedFixture, Vec<Diagnostic>> {
    let error = |message: String| vec![solved_test_diagnostic(message, description.origin)];
    let declaring = description
        .origin
        .map(|loc| sources.get(loc.source).name.to_string())
        .ok_or_else(|| error("fixture has no declaring module".into()))?;
    let host = context
        .host
        .as_deref_mut()
        .ok_or_else(|| error("fixture source host is unavailable".into()))?;
    let source = host
        .resolve(
            &declaring,
            context
                .module_paths
                .get(&declaring)
                .map(|path| path.as_path()),
            label,
        )
        .map_err(error)?;
    let text = cache
        .entry(source.key.clone())
        .or_insert_with(|| host.read(&source, limits.file_size))
        .as_ref()
        .map_err(|message| error(message.clone()))?;
    if text.len() > limits.file_size {
        return Err(error("fixture file size limit exceeded".into()));
    }
    *retained = retained.saturating_add(text.len());
    if *retained > context.limits.fixture_bytes {
        return Err(error("aggregate fixture budget exceeded".into()));
    }
    let name = format!(
        "@test-ctx/{}/{}/{}",
        solved_test_encode(module),
        solved_test_encode(&case.name),
        case.fixtures
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join("/")
    );
    let id = sources.add(name, text);
    let plan = match source.format {
        crate::SystemDataFormat::Json => crate::json::validate_json_registered(sources, id),
        crate::SystemDataFormat::Yaml => crate::yaml::validate_yaml_registered(sources, id),
        crate::SystemDataFormat::Toml => crate::toml::validate_toml_registered(sources, id),
    }?;
    let stats = plan
        .enforce_limits(limits, text.len())
        .map_err(|err| error(err.to_string()))?;
    let bytes = stats
        .nodes
        .saturating_mul(64)
        .saturating_add(stats.payloads_bytes);
    *retained = retained.saturating_add(bytes);
    if *retained > context.limits.fixture_bytes {
        return Err(error("aggregate fixture budget exceeded".into()));
    }
    Ok(SolvedFixture {
        plan,
        bytes: u64::try_from(bytes).unwrap_or(u64::MAX),
    })
}

fn solved_test_encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").unwrap();
        }
    }
    encoded
}

fn record_solved_test_notice(
    report: &mut crate::test_plan::TestReport,
    result: &mut crate::test_plan::TestResult,
) {
    if !result.diagnostics.is_empty() {
        report.notices.push(crate::test_plan::TestNotice {
            before_case: report.cases.len(),
            context: result.clone(),
        });
        result.diagnostics.clear();
    }
}

fn fail_solved_test(
    report: &mut crate::test_plan::TestReport,
    mut result: crate::test_plan::TestResult,
    message: &str,
    origin: Option<crate::Loc>,
    terminal: bool,
) {
    result
        .diagnostics
        .push(solved_test_diagnostic(message, origin));
    report.cases.push(result);
    report.aborted |= terminal;
}

fn solved_test_diagnostic(message: impl Into<String>, origin: Option<crate::Loc>) -> Diagnostic {
    Diagnostic {
        severity: crate::source::Severity::Error,
        message: message.into(),
        labels: origin
            .map(|location| crate::source::Label {
                location,
                message: String::new(),
                primary: true,
            })
            .into_iter()
            .collect(),
        notes: vec![],
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
