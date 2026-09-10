struct PreparedToolExpression {
    expression: crate::compiler::PreparedExternalExpression,
    source: crate::SourceId,
    // Populated only by the execution consumer, without type inference.
    function: std::sync::OnceLock<Result<crate::BytecodeFunction, FrontendError>>,
    required: Vec<String>,
    // Binding name, solved graph root, and type-family arity.
    runtime_types: Vec<(String, AnalysisTypeId, usize)>,
}

struct SolvedToolBinding<'a> {
    binding: &'a Binding,
    plan: PreparedToolExpression,
}

// A definition has one tool-stage value, even when several consumers need it.
// Keep completion on the task, not in the name-indexed linking environment.
struct ToolBindingTask<'a> {
    binding: &'a Binding,
    plan: PreparedToolExpression,
    value: Option<Val>,
}

impl ToolBindingTask<'_> {
    fn execute(
        &mut self,
        source_name: &str,
        bindings: &BTreeMap<String, Val>,
        account: &mut QuotaAccount,
        sources: &SourceDatabase,
        evaluator: &mut ToolEvaluator<'_>,
    ) -> Result<Val, FrontendError> {
        if let Some(value) = self.value {
            return Ok(value);
        }
        let value = evaluate_prepared_tool_expression(
            source_name, bindings, &self.plan, account, sources, evaluator, false,
        )?;
        self.value = Some(value);
        Ok(value)
    }
}

// Reuse the module solver's completed evidence; do not infer the binding again.
fn prepare_solved_tool_binding(
    binding: &Binding,
    inference: &GenericInference<'_>,
    publication: &mut InferencePublication<'_>,
    context: &mut ToolInferenceContext<'_>,
    sources: &SourceDatabase,
) -> Result<PreparedToolExpression, String> {
    fn records<V: Clone>(
        map: &HashMap<crate::Location, V>, range: crate::Location,
    ) -> HashMap<crate::Location, V> {
        map.iter().filter(|(location, _)| location.source == range.source
            && range.start <= location.start && location.end <= range.end)
            .map(|(location, value)| (*location, value.clone())).collect()
    }
    let expression = &binding.value.value;
    let range = expression.location;
    let inputs = HirProgram::resolve_expression(expression, Vec::new());
    let mut evidence = ToolExpressionEvidence {
        external_names: tool_external_names(expression, &inputs, context),
        value_constructors: records(&inference.value_constructors, range),
        calls: records(&inference.resolved_call_evidence, range),
        inferred_scopes: records(&inference.inferred_runtime_scopes, range),
        families: records(&inference.propagation_families, range),
        not_families: records(&inference.not_families, range),
        members: records(&inference.resolved_trait_members, range),
        interpolations: records(&inference.resolved_interpolation_evidence, range),
        ..Default::default()
    };
    if let Some(scheme) = context.schemes.get(&binding.value.name.value) {
        let names = scheme.constraints.iter().enumerate().map(|(index, constraint)| {
            let name = evidence_parameter_name(&binding.value.name.value, index);
            if constraint.capability == TypeCapability::RuntimeType {
                evidence.lexical_types.insert(constraint.parameter, name.clone());
            }
            name
        }).collect();
        evidence.parameters.insert(range, names);
    }
    evidence.parameters.extend(evidence.inferred_scopes.iter().map(|(location, entries)|
        (*location, entries.iter().map(|entry| entry.name.clone()).collect())));
    evidence.expression_types = records(&inference.records, range).into_iter().map(|(location, slot)| {
        (location, publication.publish_tool_root(&mut context.types, slot,
            |slot| inference.normalize(&TypeDescriptor::Inference(slot))))
    }).collect();
    let mut required = BTreeSet::new();
    for entry in evidence.calls.values().flatten().chain(evidence.members.values()).chain(evidence.interpolations.values()) {
        entry.collect_bindings(&mut required);
    }
    for name in required {
        if let Some(descriptor) = inference.runtime_type_evidence.get(&name) {
            let descriptor = inference.normalize(descriptor);
            let root = context.types.intern_resolved_descriptor(&descriptor)
                .ok_or_else(|| format!("runtime type evidence {name:?} has no solved static type"))?;
            evidence.runtime_types.insert(name, root);
        }
    }
    prepare_tool_execution(expression, evidence, sources, &mut context.types)
}

#[cfg(test)]
mod tool_plan_tests {
    use super::*;
    use crate::heap::DecodedValue;

    #[test]
    fn property_plan_solves_previous_statically_and_restores_inputs_on_error() {
        for valid in [true, false] {
            let mut sources = SourceDatabase::default();
            let source = sources.add("static-property-plan", "provider(previous)");
            let mut program = parse_registered(&sources, source).program.unwrap();
            let ExprKind::Call { arguments, .. } = &mut program.value.body.value.result.value else {
                panic!("expected call");
            };
            let ExprKind::Variable(name) = &mut arguments[0].value else { panic!("expected variable"); };
            name.value = PROPERTY_PREVIOUS_BINDING.into();
            let prelude = BootstrapPrelude::new();
            let hir = HirProgram::resolve(&program, prelude.types.keys().cloned()
                .chain(["provider".into(), PROPERTY_PREVIOUS_BINDING.into()]));
            let mut context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
                prelude.schemes, BTreeMap::new(), true);
            let before = context.environment.clone();
            let environment = HashMap::from([("provider".into(), TypeDescriptor::Function {
                parameters: vec![option_descriptor(TypeDescriptor::Int)],
                result: Box::new(if valid { TypeDescriptor::Int } else { TypeDescriptor::String }),
            })]);
            let plan = prepare_property_call(&program.value.body.value.result, &TypeDescriptor::Int,
                &environment, None, &sources, &mut context);
            assert_eq!(plan.is_ok(), valid, "{:?}", plan.as_ref().err());
            assert_eq!(context.environment, before);
            if let Ok(plan) = plan {
                assert!(plan.required.iter().any(|name| name == PROPERTY_PREVIOUS_BINDING));
                assert!(plan.required.iter().any(|name| name == "provider"));
            }
        }
    }

    #[test]
    fn member_aliases_compile_from_hir_without_ready_runtime_values() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("member-alias-plan", "import PropertyTarget.{Type as OnType}; OnType");
        let program = parse_registered(&sources, source).program.unwrap();
        let prelude = BootstrapPrelude::new();
        let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
        let mut context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
            prelude.schemes, BTreeMap::new(), true);
        assert!(!context.environment.contains_key("OnType"));
        let expression = &program.value.body.value.result;
        let evidence = solve_tool_expression_types(expression, Some(&property_target_descriptor()), None,
            &sources, &mut context).unwrap();
        let plan = prepare_tool_execution(expression, evidence, &sources, &mut context.types).unwrap();
        assert!(plan.required.iter().any(|name| name == "OnType"));
    }

    #[test]
    fn multiple_plans_share_type_ids_and_execute_after_arena_publication() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("shared-tool-types", "witness");
        let program = parse_registered(&sources, source).program.unwrap();
        let mut prelude = BootstrapPrelude::new();
        prelude.types.insert("witness".into(), TypeDescriptor::Type);
        let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
        let descriptor = TypeDescriptor::Array(Box::new(TypeDescriptor::Int));
        let mut module_types = TypeGraph::default();
        let unrelated = module_types.intern_descriptor(&TypeDescriptor::String);
        let existing_root = module_types.intern_descriptor(&descriptor);
        let storage = module_types.nodes.as_ptr();
        let mut context = ToolInferenceContext::new(module_types, &hir, BTreeMap::new(), prelude.types,
            prelude.schemes, BTreeMap::new(), true);
        assert_eq!(context.types.nodes.as_ptr(), storage, "handoff must move the arena without copying it");
        let expression = &program.value.body.value.result;
        let mut plans = Vec::new();
        let mut counts = Vec::new();
        for _ in 0..2 {
            let mut evidence = solve_tool_expression_types(expression, Some(&TypeDescriptor::Type), None,
                &sources, &mut context).unwrap();
            let root = context.types.intern_descriptor(&descriptor);
            assert_eq!(root, existing_root, "plans must consume the existing module ID");
            evidence.runtime_types.insert("witness".into(), root);
            plans.push(prepare_tool_execution(expression, evidence, &sources, &mut context.types).unwrap());
            counts.push(context.types.nodes().len());
        }
        assert_eq!(counts[0], counts[1], "second plan must reuse existing nodes");
        assert_eq!(plans[0].runtime_types[0].1, plans[1].runtime_types[0].1);
        let mut main = Heap::main();
        let mut evaluator = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut main);
        evaluator.tool_types = context.types;
        let mut account = QuotaAccount::new(Quota::with_fuel(1000));
        let mut previous = None;
        for plan in plans {
            let value = evaluate_prepared_tool_expression("shared-tool-types", &BTreeMap::new(),
                &plan, &mut account, &sources, &mut evaluator, false).unwrap();
            if let Some(previous) = previous {
                assert_eq!(value.value(), previous, "shared graph roots must reuse materialized values");
            }
            previous = Some(value.value());
            let (graph, root) = evaluator.decode_type_graph(value, "shared type witness").unwrap();
            assert_eq!(graph.descriptor(root).unwrap(), descriptor);
        }
        let module_types = std::mem::take(&mut evaluator.tool_types);
        assert_eq!(module_types.descriptor(existing_root).unwrap(), descriptor);
        assert_eq!(module_types.descriptor(unrelated).unwrap(), TypeDescriptor::String);
    }

    #[test]
    fn compilation_discards_unlinked_type_evidence_without_rebuilding_shared_graph() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("unlinked-tool-types", "41 + 1");
        let program = parse_registered(&sources, source).program.unwrap();
        let prelude = BootstrapPrelude::new();
        let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
        let mut context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
            prelude.schemes, BTreeMap::new(), true);
        let expression = &program.value.body.value.result;
        let mut evidence = solve_tool_expression_types(expression, Some(&TypeDescriptor::Int), None,
            &sources, &mut context).unwrap();
        assert!(context.types.nodes().len() > 0);
        let unused = context.types.intern_descriptor(&TypeDescriptor::Named("unlinked".into()));
        evidence.runtime_types.insert("unused-type-evidence".into(), unused);
        let plan = prepare_tool_execution(expression, evidence, &sources, &mut context.types).unwrap();
        assert!(plan.runtime_types.is_empty());
        assert!(matches!(context.types.node(unused), TypeNode::Named(name) if name == "unlinked"));
        let mut main = Heap::main();
        let mut evaluator = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut main);
        evaluator.tool_types = context.types;
        let mut account = QuotaAccount::new(Quota::with_fuel(1000));
        let value = evaluate_prepared_tool_expression("unlinked-tool-types", &BTreeMap::new(),
            &plan, &mut account, &sources, &mut evaluator, false).unwrap();
        assert!(matches!(value.value(), DecodedValue::Int(42)));
    }

    #[test]
    fn property_module_plans_prepare_without_capability_or_provider_values() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("static-property-module", r#"
            @property(PropertyTarget.Type) type Tag = struct(Int);
            def tag: Fn(Type, Option(Tag)) -> Tag = fn(target, previous) { fail!("must not run") };
            @tag type Item = struct(Int);
            0
        "#);
        let program = parse_registered(&sources, source).program.unwrap();
        let declared = |name: &str, local| TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::applied(crate::ModuleId::ANONYMOUS, local, &[]),
            name: name.into(),
            body: Arc::new(TypeDescriptor::Newtype(Box::new(TypeDescriptor::Int))),
        });
        let tag = declared("Tag", 990);
        let mut prelude = BootstrapPrelude::new();
        prelude.types.extend([
            ("Tag".into(), TypeDescriptor::TypeOf(Box::new(tag.clone()))),
            ("Item".into(), TypeDescriptor::TypeOf(Box::new(declared("Item", 991)))),
            ("tag".into(), TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Type, option_descriptor(tag.clone())],
                result: Box::new(tag.clone()),
            }),
        ]);
        let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
        let environment = prelude.types.clone();
        let mut context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
            prelude.schemes, BTreeMap::new(), true);
        let missing = prepare_property_plans(&program, &HashMap::new(), &environment, None, &sources, &mut context);
        assert!(missing.err().unwrap().message.contains("no solved contract"));
        let contracts = program.value.body.value.bindings.iter()
            .flat_map(|binding| &binding.value.decorators)
            .filter(|decorator| !intrinsic_property_marker(decorator))
            .map(|decorator| (decorator.location, tag.clone())).collect();
        let plans = prepare_property_plans(&program, &contracts, &environment, None, &sources, &mut context).unwrap();
        assert_eq!(plans.capabilities.len(), 1);
        assert_eq!(plans.decorators.len(), 1);
        let decorator = plans.decorators.values().next().unwrap();
        assert!(decorator.plan.required.iter().any(|name| name == "tag"));
        assert!(decorator.plan.required.iter().any(|name| name == PROPERTY_PREVIOUS_BINDING));
    }

    #[test]
    fn construction_plans_validate_without_vm_and_execute_without_inference() {
        for valid in [true, false] {
            let mut sources = SourceDatabase::default();
            let body = if valid { "fail!(\"callback must not run\")" } else { "\"wrong\"" };
            let source = sources.add("static-check-plan",
                &format!("@check(fn(x) {{ {body} }}) type Item = struct(Int); 0"));
            let program = parse_registered(&sources, source).program.unwrap();
            let declared = TypeDescriptor::Declared(DeclaredTypeDescriptor {
                id: crate::value::DeclaredTypeId::applied(crate::ModuleId::ANONYMOUS, 990, &[]),
                name: "Item".into(),
                body: Arc::new(TypeDescriptor::Newtype(Box::new(TypeDescriptor::Int))),
            });
            let environment = HashMap::from([("Item".into(), TypeDescriptor::TypeOf(Box::new(declared)))]);
            let prelude = BootstrapPrelude::new();
            let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
            let mut context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
                prelude.schemes, BTreeMap::new(), true);
            let plans = prepare_construction_checks(&program, &environment, None, &sources, &mut context);
            assert_eq!(plans.is_ok(), valid, "{:?}", plans.as_ref().err());
            if let Ok(plans) = plans {
                assert_eq!(plans.len(), 1);
                let mut main = Heap::main();
                let mut evaluator = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut main);
                evaluator.tool_types = context.types;
                let mut account = QuotaAccount::new(Quota::with_fuel(1000));
                evaluate_construction_checks("static-check-plan", &plans, &BTreeMap::new(),
                    &mut account, &sources, &mut evaluator, true).unwrap();
                assert!(evaluator.construction_checks_complete);
                assert_eq!(evaluator.registered_construction_checks.len(), 1);
            }
        }
    }

    #[test]
    fn binding_task_retries_missing_inputs_and_executes_successfully_only_once() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("tool-definition-once", "def output = input + 1; output");
        let program = parse_registered(&sources, source).program.unwrap();
        let binding = &program.value.body.value.bindings[0];
        let mut prelude = BootstrapPrelude::new();
        prelude.types.insert("input".into(), TypeDescriptor::Int);
        let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
        let mut context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
            prelude.schemes, BTreeMap::new(), true);
        let expression = &binding.value.value;
        let evidence = solve_tool_expression_types(expression, Some(&TypeDescriptor::Int), None,
            &sources, &mut context).unwrap();
        let plan = prepare_tool_execution(expression, evidence, &sources, &mut context.types).unwrap();
        let mut task = ToolBindingTask { binding, plan, value: None };
        assert!(task.plan.function.get().is_none(), "static preparation must not generate bytecode");
        let mut main = Heap::main();
        let mut evaluator = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut main);
        evaluator.tool_types = context.types;
        let mut account = QuotaAccount::new(Quota::with_fuel(1000));
        let error = task.execute("tool-definition-once", &BTreeMap::new(),
            &mut account, &sources, &mut evaluator).unwrap_err();
        assert!(error.message.contains("unavailable binding"), "{}", error.message);
        assert!(task.value.is_none());
        let bindings = BTreeMap::from([("input".into(), Val::unknown(DecodedValue::Int(41)))]);
        let value = task.execute("tool-definition-once", &bindings,
            &mut account, &sources, &mut evaluator).unwrap();
        assert!(matches!(value.value(), DecodedValue::Int(42)));
        let compiled = task.plan.function.get().expect("execution compiles the plan") as *const _;
        let repeated = evaluate_prepared_tool_expression("tool-definition-once", &bindings,
            &task.plan, &mut account, &sources, &mut evaluator, false).unwrap();
        assert!(matches!(repeated.value(), DecodedValue::Int(42)));
        assert_eq!(task.plan.function.get().unwrap() as *const _, compiled);
        // The later ordinary task consumes the completed definition even if its
        // linking environment has changed. No execution fuel is required.
        let mut no_fuel = QuotaAccount::new(Quota::with_fuel(0));
        let value = task.execute("tool-definition-once", &BTreeMap::new(),
            &mut no_fuel, &sources, &mut evaluator).unwrap();
        assert!(matches!(value.value(), DecodedValue::Int(42)));
    }

    #[test]
    fn compiled_plan_runs_without_an_inference_context_and_reports_missing_links() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("compiled-tool-plan", "input + 1");
        let program = parse_registered(&sources, source).program.unwrap();
        for linked in [true, false] {
            let (plan, types) = {
                let mut prelude = BootstrapPrelude::new();
                prelude.types.insert("input".into(), TypeDescriptor::Int);
                let hir = HirProgram::resolve(&program, prelude.types.keys().cloned());
                let mut context = ToolInferenceContext::new(TypeGraph::default(), &hir, BTreeMap::new(), prelude.types,
                    prelude.schemes, BTreeMap::new(), true);
                let expression = &program.value.body.value.result;
                let evidence = solve_tool_expression_types(expression, Some(&TypeDescriptor::Int), None,
                    &sources, &mut context).unwrap();
                let plan = prepare_tool_execution(expression, evidence, &sources, &mut context.types).unwrap();
                (plan, context.types)
            };
            assert_eq!(plan.required, vec!["input"]);
            let mut main = Heap::main();
            let mut evaluator = ToolEvaluator::new(Arc::new(DiscardDebugSink), &mut main);
            evaluator.tool_types = types;
            let bindings = if linked { BTreeMap::from([("input".into(), Val::unknown(DecodedValue::Int(41)))]) }
                else { BTreeMap::new() };
            let mut account = QuotaAccount::new(Quota::with_fuel(1000));
            let result = evaluate_prepared_tool_expression("compiled-tool-plan", &bindings, &plan,
                &mut account, &sources, &mut evaluator, false);
            if linked {
                assert!(matches!(result.unwrap().value(), DecodedValue::Int(42)));
            } else {
                assert!(result.unwrap_err().message.contains("unavailable binding \"input\""));
            }
            let rebound = BTreeMap::from([("input".into(), Val::unknown(DecodedValue::Int(99)))]);
            let result = evaluate_prepared_tool_expression("compiled-tool-plan", &rebound, &plan,
                &mut account, &sources, &mut evaluator, false).unwrap();
            assert!(matches!(result.value(), DecodedValue::Int(100)));
        }
    }

    #[test]
    fn owner_parameters_are_closed_before_execution_without_runtime_resources() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("static-owner-plan", "owner");
        let program = parse_registered(&sources, source).program.unwrap();
        let expression = &program.value.body.value.result;
        let parameter = TypeParameterId(7);
        let argument = TypeDescriptor::Bound(parameter);
        let descriptor = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::applied(
                crate::ModuleId::ANONYMOUS,
                990,
                &[argument.clone()],
            ),
            name: "Owner".into(),
            body: Arc::new(TypeDescriptor::Newtype(Box::new(argument))),
        });
        let mut evidence = ToolExpressionEvidence::default();
        let mut types = TypeGraph::default();
        evidence.external_names.extend(["owner".into(), "captured_type".into()]);
        let root = types.intern_descriptor(&descriptor);
        evidence
            .expression_types
            .insert(expression.location, ToolTypeRoot::Graph(root));
        evidence
            .lexical_types
            .insert(parameter, "captured_type".into());
        let plan = prepare_tool_execution(expression, evidence, &sources, &mut types).unwrap();
        assert_eq!(plan.runtime_types.len(), 1);
        let (_, root, arity) = &plan.runtime_types[0];
        assert_eq!(*arity, 1);
        let descriptor = types.descriptor(*root).unwrap();
        let mut parameters = Vec::new();
        collect_bound_parameters(&descriptor, &mut parameters);
        assert!(!parameters.is_empty());
        assert!(parameters.iter().all(|parameter| parameter.index() == 0));
        assert!(plan.required.iter().any(|name| name == "captured_type"));
    }
}

// This plan is independent of VM state and contains no inference capability.
fn prepare_tool_execution(
    expression: &Expr,
    evidence: ToolExpressionEvidence,
    sources: &SourceDatabase,
    types: &mut TypeGraph,
) -> Result<PreparedToolExpression, String> {
    let ToolExpressionEvidence {
        mut external_names,
        expression_types,
        value_constructors,
        calls,
        runtime_types,
        parameters,
        lexical_types,
        inferred_scopes,
        families,
        not_families,
        members,
        interpolations,
    } = evidence;
    let mut evidence_names = BTreeSet::new();
    for evidence in calls.values().flatten().chain(members.values()).chain(interpolations.values()) {
        evidence.collect_bindings(&mut evidence_names);
    }
    let arities = expression_types
        .iter()
        .filter_map(|(location, root)| root.arity(&types).map(|arity| (*location, arity)))
        .collect();
    let mut lowered = expression.clone();
    crate::elaboration::elaborate_tool_expression(
        &mut lowered,
        &calls,
        &arities,
        &parameters,
        &families,
        &not_families,
        &members,
        &interpolations,
    );
    let mut runtime_types = runtime_types
        .into_iter()
        .map(|(name, root)| {
            let arity = if name.starts_with("\0type_argument:") {
                ToolTypeRoot::Graph(root).bound_arity(&types)
            } else {
                0
            };
            (name, root, arity)
        })
        .collect::<Vec<_>>();
    let mut declared_value_owners = HashMap::new();
    for (location, root) in &expression_types {
        let Some(owner_root) = root.owner(&types, value_constructors.contains_key(location)) else {
            continue;
        };
        if location.source != expression.location.source
            || location.start < expression.location.start
            || location.end > expression.location.end
        {
            continue;
        }
        let descriptor = owner_root.descriptor(&types)?;
        let key = if type_identity_is_symbolic(&descriptor) {
            format!("\0owner-family:{}:{}", location.start, location.end)
        } else {
            crate::compiler::declared_owner_link_key(*location)
        };
        let mut owner = ResolvedEvidence::root(key.clone());
        let mut parameters = Vec::new();
        collect_bound_parameters(&descriptor, &mut parameters);
        parameters.sort_by_key(|parameter| parameter.0);
        parameters.dedup();
        let mut replacements = HashMap::new();
        for (index, parameter) in parameters.iter().enumerate() {
            let inferred = inferred_scopes
                .iter()
                .filter(|(scope, _)| {
                    scope.source == location.source
                        && scope.start <= location.start
                        && location.end <= scope.end
                })
                .filter_map(|(scope, evidence)| {
                    evidence
                        .iter()
                        .find(|evidence| evidence.target == TypeDescriptor::Bound(*parameter))
                        .map(|evidence| (scope.end - scope.start, &evidence.name))
                })
                .min_by_key(|(length, _)| *length)
                .map(|(_, name)| name);
            let Some(name) = inferred.or_else(|| lexical_types.get(parameter)) else {
                break;
            };
            owner.arguments.push(ResolvedEvidence::root(name.clone()));
            replacements.insert(
                *parameter,
                TypeDescriptor::Bound(TypeParameterId(index as u32)),
            );
        }
        if owner.arguments.len() != parameters.len() {
            continue;
        }
        let descriptor = substitute_bound_parameters(&descriptor, &replacements);
        let root = types
            .intern_resolved_descriptor(&descriptor)
            .ok_or("tool owner has no solved static type")?;
        runtime_types.push((key, root, owner.arguments.len()));
        declared_value_owners.insert(*location, owner);
    }
    for evidence in declared_value_owners.values() { evidence.collect_bindings(&mut evidence_names); }
    external_names.extend(evidence_names);
    external_names.extend(runtime_types.iter().map(|(name, _, _)| name.clone()));
    let source = sources.get(expression.location.source);
    let (prepared, required) = prepare_expression_with_external_bindings(
        lowered, |name| external_names.contains(name), declared_value_owners, value_constructors, source)
        .map_err(|error| error.to_string())?;
    // Static preparation returns sorted external links. Their witnesses survive
    // into execution; bytecode generation needs no type queries.
    runtime_types.retain(|(name, _, _)| required.binary_search(name).is_ok());
    Ok(PreparedToolExpression {
        expression: prepared,
        source: expression.location.source,
        function: std::sync::OnceLock::new(),
        required,
        runtime_types,
    })
}
