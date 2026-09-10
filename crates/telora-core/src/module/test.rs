#[derive(Clone, Debug)]
pub(crate) struct TestDescription {
    pub(crate) kind: TestKind,
    pub(crate) expected: Option<String>,
    pub(crate) sources: Vec<String>,
    pub(crate) origin: Option<crate::Loc>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TestKind {
    ShouldOk,
    ShouldFail,
    ShouldFailWith,
    Fixtures,
}

pub(crate) const TEST_NATIVE_TYPE: crate::value::NativeTypeId = crate::value::NativeTypeId {
    module: crate::value::NativeModuleId(33),
    local: 0,
};

/// Finite bounds on deferred expansion and Host-retained fixture data.
#[derive(Clone, Copy, Debug)]
pub struct TestLimits {
    pub cases: usize,
    pub depth: usize,
    pub fixture_bytes: usize,
}

impl Default for TestLimits {
    fn default() -> Self {
        Self {
            cases: 10_000,
            depth: 64,
            fixture_bytes: 256 * 1024 * 1024,
        }
    }
}

/// A private Host key, never a module identity or a public diagnostic label.
#[derive(Clone, Debug)]
pub struct TestSource {
    pub key: String,
    pub format: SystemDataFormat,
}

pub trait TestHost {
    fn resolve(
        &mut self,
        declaring_module: &str,
        declaring_path: Option<&Path>,
        source: &str,
    ) -> Result<TestSource, String>;
    fn read(&mut self, source: &TestSource, max_bytes: usize) -> Result<String, String>;
}

#[derive(Clone, Debug)]
pub struct TestCase {
    pub test: String,
    pub fixtures: Vec<usize>,
    pub sources: Vec<String>,
    pub phase: &'static str,
    pub passed: bool,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct TestOutcome {
    pub module: String,
    pub sources: SourceDatabase,
    pub diagnostics: Vec<Diagnostic>,
    pub cases: Vec<TestCase>,
    /// Factory diagnostics may belong to a group with no leaf record of its own.
    pub notices: Vec<TestNotice>,
    pub aborted: bool,
}

pub struct TestNotice {
    pub before_case: usize,
    pub context: TestCase,
}

impl TestOutcome {
    pub fn passed(&self) -> bool {
        !self.aborted && !self.cases.is_empty() && self.cases.iter().all(|case| case.passed)
    }
}

impl Engine {
    pub fn test_with_resolver(
        &self,
        resolver: ModuleResolver,
        host: &mut dyn TestHost,
        limits: TestLimits,
    ) -> Result<TestOutcome, ModuleError> {
        let resolver = resolver.with_builtins(builtin_list());
        let root = resolver
            .selected_root()
            .map_err(|e| ModuleError::new(e.to_string()))?;
        let mut sources = SourceDatabase::default();
        let graph = ModuleGraph::discover(
            &resolver,
            vec![root.clone()],
            &BTreeMap::new(),
            builtin_list()
                .into_iter()
                .map(|(name, _)| ModuleCName::builtin(name)),
            None,
            true,
            &mut sources,
        )?;
        let mut resolved = StaticNames::new(&graph).resolve_all();
        if let Some(inputs) = resolved.diagnostic_inputs(&graph) {
            let snapshot = WorkspaceSnapshot::build(sources.clone(), inputs);
            return Ok(TestOutcome {
                module: root.id.to_string(), sources,
                diagnostics: snapshot.diagnostics().to_vec(),
                cases: Vec::new(), notices: Vec::new(), aborted: true,
            });
        }
        let mut main = MainWorld::from_resolved(graph, resolved);
        let builtin_modules = install_native_modules(&mut main, &mut sources, &self.debug_sink)?;
        let mut builder = WorkspaceBuilder {
            engine: self,
            overlays: &BTreeMap::new(),
            query: None,
            sources,
            main,
            builtin_modules,
            inputs: BTreeMap::new(),
            provenances: HashMap::new(),
            roots: HashMap::new(),
            interfaces: HashMap::new(),
            visiting: Vec::new(),
            cycle_members: HashSet::new(),
            cycle_reported: false,
        };
        block_on_recovery(builder.load_telora(root.clone()));
        let snapshot = WorkspaceSnapshot::build_borrowed(
            builder.sources.clone(),
            builder.inputs.values(),
        );
        let diagnostics = snapshot.diagnostics().to_vec();
        let mut outcome = TestOutcome {
            module: root.id.to_string(),
            sources: builder.sources.clone(),
            diagnostics,
            cases: Vec::new(),
            notices: Vec::new(),
            aborted: false,
        };
        if outcome
            .diagnostics
            .iter()
            .any(|d| d.severity == crate::source::Severity::Error)
        {
            outcome.aborted = true;
            return Ok(outcome);
        }
        let Some(module_root) = builder.roots.get(&root.id).copied() else {
            outcome.aborted = true;
            outcome.diagnostics.push(test_diagnostic(
                "test module could not be initialized",
                None,
            ));
            return Ok(outcome);
        };
        let interface = &builder.interfaces[&root.id];
        let selected = interface
            .exports
            .iter()
            .filter(|(_, scheme)| {
                scheme.parameters.is_empty()
                    && matches!(&scheme.body,
                TypeDescriptor::Opaque(native) if native.id() == TEST_NATIVE_TYPE)
            })
            .map(|(name, _)| name.clone())
            .collect::<BTreeSet<_>>();
        if selected.is_empty() {
            outcome.aborted = true;
            outcome.diagnostics.push(test_diagnostic(
                "test module has no direct Test exports",
                None,
            ));
            return Ok(outcome);
        }
        let (value_owner, _) =
            semantic_value_contract(&builder.builtin_modules, &builder.main.heap)?;
        let module_paths = builder
            .inputs
            .values()
            .filter_map(|input| input.path.clone().map(|path| (input.key.clone(), path)))
            .collect();
        let main = builder.main.seal();
        let module = WorkWorld::new(Heap::work_for(&main.heap), module_root.runtime());
        let mut runner = TestRunner {
            engine: self,
            main: &main.heap,
            host,
            limits,
            account: QuotaAccount::new(self.config.session_quota).with_sources(&builder.sources),
            retained: 0,
            expanded: 0,
            value_owner: value_owner.runtime(),
            module_paths,
            outcome: &mut outcome,
        };
        for name in selected {
            let value = module
                .module_member_ref(&main.heap, &name)
                .map_err(|e| ModuleError::new(e.to_string()))?
                .ok_or_else(|| ModuleError::new("Test export has no runtime value"))?
                .runtime();
            let world = WorkWorld::new(Heap::work_for(&main.heap), value);
            runner.run(
                world,
                TestCase {
                    test: name,
                    fixtures: Vec::new(),
                    sources: Vec::new(),
                    phase: "discovery",
                    passed: false,
                    diagnostics: Vec::new(),
                },
                0,
            );
            if runner.outcome.aborted {
                break;
            }
        }
        Ok(outcome)
    }
}

fn test_diagnostic(message: impl Into<String>, origin: Option<crate::Loc>) -> Diagnostic {
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
        notes: Vec::new(),
    }
}

struct TestRunner<'a> {
    engine: &'a Engine,
    main: &'a Heap,
    host: &'a mut dyn TestHost,
    limits: TestLimits,
    account: QuotaAccount,
    retained: usize,
    expanded: usize,
    value_owner: Val,
    module_paths: HashMap<String, PathBuf>,
    outcome: &'a mut TestOutcome,
}

impl TestRunner<'_> {
    fn fail(
        &mut self,
        mut case: TestCase,
        message: impl Into<String>,
        origin: Option<crate::Loc>,
        terminal: bool,
    ) {
        case.diagnostics.push(test_diagnostic(message, origin));
        self.outcome.cases.push(case);
        self.outcome.aborted |= terminal;
    }

    fn run(&mut self, world: WorkWorld, mut case: TestCase, depth: usize) {
        self.expanded += 1;
        if self.expanded > self.limits.cases || depth > self.limits.depth {
            self.fail(case, "test expansion limit exceeded", None, true);
            return;
        }
        let Some((description, callable)) = world.root_ref(self.main).test_description() else {
            self.fail(case, "expected runtime std/test.Test witness", None, true);
            return;
        };
        let description = description.clone();
        if description.kind == TestKind::Fixtures {
            self.group(world, case, description, depth);
            return;
        }
        case.phase = "execution";
        let mut world = world;
        world.set_root(callable);
        let result = self.call(world, &[]);
        let diagnostics = self.account.take_diagnostics();
        let terminal = result
            .as_ref()
            .err()
            .is_some_and(|e| e.failure_class() == crate::evaluation::FailureClass::Terminal);
        let failure = result.as_ref().err();
        case.passed = !terminal
            && match description.kind {
                TestKind::ShouldOk => result.is_ok(),
                TestKind::ShouldFail => result.is_err(),
                TestKind::ShouldFailWith => failure.is_some_and(|e| {
                    e.message
                        .contains(description.expected.as_deref().unwrap_or(""))
                }),
                TestKind::Fixtures => unreachable!(),
            };
        case.diagnostics.extend(diagnostics.into_iter().filter(|d| {
            !case.passed
                || description.kind == TestKind::ShouldOk
                || d.severity != crate::source::Severity::Error
        }));
        if !case.passed {
            if let Some(error) = failure {
                let diagnostic = error
                    .diagnostic()
                    .unwrap_or_else(|| test_diagnostic(&error.message, description.origin));
                if !case.diagnostics.contains(&diagnostic) {
                    case.diagnostics.push(diagnostic);
                }
            }
            if description.kind != TestKind::ShouldOk && !terminal {
                case.diagnostics.push(test_diagnostic(
                    match description.expected {
                        Some(expected) => {
                            format!("expected a recoverable failure containing {expected:?}")
                        }
                        None => {
                            "expected a recoverable failure, but thunk returned normally".into()
                        }
                    },
                    description.origin,
                ));
            }
        }
        self.outcome.aborted |= terminal;
        self.outcome.cases.push(case);
    }

    fn call(
        &mut self,
        world: WorkWorld,
        arguments: &[Val],
    ) -> Result<WorkWorld, crate::RuntimeError> {
        let wrapper = BytecodeFunction::with_signature(
            "<test callable>",
            arguments.len() + 1,
            0,
            arguments.len() + 1,
            Vec::new(),
            vec![
                Instruction::Call {
                    base: Register(0),
                    argument_count: arguments.len(),
                },
                Instruction::Return { src: Register(0) },
            ],
        );
        Vm::new()
            .with_debug_sink(Arc::clone(&self.engine.debug_sink))
            .execute_in_existing_world_with_runtime_args(
                self.main,
                &HashMap::new(),
                &wrapper,
                world,
                arguments,
                &[],
                &mut self.account,
            )
    }

    fn group(
        &mut self,
        world: WorkWorld,
        case: TestCase,
        description: TestDescription,
        depth: usize,
    ) {
        if description.sources.is_empty() {
            self.fail(case, "no fixtures", description.origin, false);
            return;
        }
        if description.sources.len() > self.limits.cases.saturating_sub(self.expanded) {
            self.fail(
                case,
                "test expansion limit exceeded",
                description.origin,
                true,
            );
            return;
        }
        let declaring = description
            .origin
            .map(|loc| self.outcome.sources.get(loc.source).name.to_string());
        let mut cache: HashMap<String, Result<String, String>> = HashMap::new();
        let mut prepared = Vec::new();
        // Fix all immediate inputs before invoking any user factory.
        for (index, label) in description.sources.iter().enumerate() {
            let mut child = case.clone();
            child.fixtures.push(index);
            child.sources.push(label.clone());
            child.phase = "fixture";
            let result = (|| -> Result<ValidatedDataPlan, Vec<Diagnostic>> {
                let error = |message: String| vec![test_diagnostic(message, description.origin)];
                let declaring = declaring
                    .as_deref()
                    .ok_or_else(|| error("fixture has no declaring module".into()))?;
                let source = self
                    .host
                    .resolve(
                        declaring,
                        self.module_paths.get(declaring).map(PathBuf::as_path),
                        label,
                    )
                    .map_err(error)?;
                let text = cache
                    .entry(source.key.clone())
                    .or_insert_with(|| {
                        self.host
                            .read(&source, self.engine.config.data_limits.file_size)
                    })
                    .as_ref()
                    .map_err(|e| error(e.clone()))?;
                let bytes = text.len();
                self.retained = self.retained.saturating_add(bytes);
                if self.retained > self.limits.fixture_bytes {
                    self.outcome.aborted = true;
                    return Err(error("aggregate fixture budget exceeded".into()));
                }
                let name = format!(
                    "@test-ctx/{}/{}/{}",
                    test_encode(&self.outcome.module),
                    test_encode(&case.test),
                    child
                        .fixtures
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join("/")
                );
                let id = self.outcome.sources.add(name, text);
                let plan = validate_system_data_source(source.format, &self.outcome.sources, id)?;
                let stats = plan
                    .enforce_limits(self.engine.config.data_limits, bytes)
                    .map_err(|e| error(e.to_string()))?;
                let cost = stats
                    .nodes
                    .saturating_mul(64)
                    .saturating_add(stats.payloads_bytes);
                self.retained = self.retained.saturating_add(cost);
                if self.retained > self.limits.fixture_bytes {
                    self.outcome.aborted = true;
                    return Err(error("aggregate fixture budget exceeded".into()));
                }
                Ok(plan)
            })();
            if self.outcome.aborted {
                child.diagnostics = result.err().unwrap_or_default();
                self.outcome.cases.push(child);
                return;
            }
            prepared.push((child, result));
        }
        for (mut child, plan) in prepared {
            if self.expanded >= self.limits.cases {
                self.fail(
                    child,
                    "test expansion limit exceeded",
                    description.origin,
                    true,
                );
                return;
            }
            let plan = match plan {
                Ok(plan) => plan,
                Err(diagnostics) => {
                    self.expanded += 1;
                    child.diagnostics = diagnostics;
                    self.outcome.cases.push(child);
                    continue;
                }
            };
            child.phase = "factory";
            let stats = plan
                .enforce_limits(self.engine.config.data_limits, 0)
                .expect("prepared fixture already admitted");
            let bytes = stats
                .nodes
                .saturating_mul(64)
                .saturating_add(stats.payloads_bytes);
            if self
                .account
                .charge_allocation(u64::try_from(bytes).unwrap_or(u64::MAX))
                .is_err()
            {
                self.fail(
                    child,
                    "fixture materialization allocation quota exceeded",
                    description.origin,
                    true,
                );
                return;
            }
            let next = WorkWorld::new(
                Heap::work_for(self.main),
                Val::unknown(DecodedValue::BuiltinAtom(BuiltinAtom::None)),
            );
            let (mut next, root) = match next.import_world_root(self.main, &world) {
                Ok(next) => next,
                Err(e) => {
                    self.fail(child, e.to_string(), description.origin, true);
                    return;
                }
            };
            next.set_root(root);
            let callable = next.root_ref(self.main).test_description().unwrap().1;
            let type_id =
                match semantic_value_type_id(next.heap(), Some(self.main), self.value_owner) {
                    Ok(id) => id,
                    Err(e) => {
                        self.fail(child, e.to_string(), description.origin, true);
                        return;
                    }
                };
            let value = materialize_data_plan(
                &plan,
                next.heap_mut(),
                Some(SemanticDataTarget {
                    background: Some(self.main),
                    type_id,
                }),
            )
            .value;
            next.set_root(callable);
            let result = self.call(next, &[value]);
            child.diagnostics.extend(self.account.take_diagnostics());
            match result {
                Ok(next) => {
                    if !child.diagnostics.is_empty() {
                        self.outcome.notices.push(TestNotice {
                            before_case: self.outcome.cases.len(),
                            context: child.clone(),
                        });
                        child.diagnostics.clear();
                    }
                    child.phase = "discovery";
                    self.run(next, child, depth + 1);
                }
                Err(error) => {
                    self.expanded += 1;
                    let terminal =
                        error.failure_class() == crate::evaluation::FailureClass::Terminal;
                    child.diagnostics.push(
                        error
                            .diagnostic()
                            .unwrap_or_else(|| test_diagnostic(&error.message, description.origin)),
                    );
                    self.outcome.cases.push(child);
                    self.outcome.aborted |= terminal;
                }
            }
            if self.outcome.aborted {
                return;
            }
        }
    }
}

fn test_encode(text: &str) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(&mut encoded, "%{byte:02X}").unwrap();
        }
    }
    encoded
}
