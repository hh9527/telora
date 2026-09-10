fn intrinsic_check_marker(decorator: &crate::ast::Decorator) -> bool {
    matches!(&decorator.value.callee.value,
        ExprKind::Variable(name) if name.value == "check")
}

fn stage_pending_construction_checks(
    module_id: crate::ModuleId,
    program: &Program,
    slots: &HashMap<crate::Location, u32>,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<(), FrontendError> {
    for binding in &program.value.body.value.bindings {
        let Some(local) = slots.get(&binding.value.name.location) else {
            continue;
        };
        let constructor = crate::TypeConstructorId {
            module: module_id,
            local: *local,
        };
        let pending = Val::unknown(crate::heap::DecodedValue::BuiltinAtom(
            crate::BuiltinAtom::None,
        ));
        if binding.value.decorators.iter().any(intrinsic_check_marker) {
            evaluator.stage_property(
                PropertyKey::Construction {
                    constructor,
                    variant: None,
                },
                pending,
            )?;
        }
        if let Some(members) = declared_member_fields(binding) {
            let mut members = members.iter().collect::<Vec<_>>();
            members.sort_by_key(|member| member.value.name.as_ref().map(|name| &name.value));
            for (index, member) in members.into_iter().enumerate() {
                if member.value.decorators.iter().any(intrinsic_check_marker) {
                    evaluator.stage_property(
                        PropertyKey::Construction {
                            constructor,
                            variant: Some(index as u32),
                        },
                        pending,
                    )?;
                }
            }
        }
    }
    Ok(())
}

struct PreparedConstructionCheck {
    key: PropertyKey,
    parameters: Vec<(String, TypeParameterId)>,
    plan: PreparedToolExpression,
}

// Static preparation has no evaluator, VM or runtime heap access. Invalid
// contracts are diagnosed here, never deferred to an execution retry.
fn prepare_construction_checks(
    program: &Program,
    environment: &HashMap<String, TypeDescriptor>,
    query: Option<crate::query::QueryContext>,
    sources: &SourceDatabase,
    context: &mut ToolInferenceContext<'_>,
) -> Result<Vec<PreparedConstructionCheck>, FrontendError> {
    let mut plans = Vec::new();
    for binding in &program.value.body.value.bindings {
        let mut checks = binding
            .value
            .decorators
            .iter()
            .filter(|decorator| intrinsic_check_marker(decorator))
            .map(|decorator| (None, decorator))
            .collect::<Vec<_>>();
        if let Some(members) = declared_member_fields(binding) {
            let mut members = members.iter().collect::<Vec<_>>();
            members.sort_by_key(|member| member.value.name.as_ref().map(|name| &name.value));
            for (index, member) in members.into_iter().enumerate() {
                checks.extend(
                    member
                        .value
                        .decorators
                        .iter()
                        .filter(|decorator| intrinsic_check_marker(decorator))
                        .map(|decorator| (Some((index as u32, member)), decorator)),
                );
            }
        }
        if checks.is_empty() {
            continue;
        }
        let reject = |message: &str, location| {
            FrontendError::from_diagnostic(sources, Diagnostic::error(message, location))
        };
        let Some(target) = environment
            .get(&binding.value.name.value)
            .and_then(constructor_instance_type)
        else {
            return Err(reject(
                "@check requires a nominal struct or payload variant",
                binding.location,
            ));
        };
        let TypeDescriptor::Declared(declared) = target else {
            return Err(reject(
                "@check requires a nominal struct or payload variant",
                binding.location,
            ));
        };
        let constructor = declared.id.constructor();
        let parameters = binding
            .value
            .type_parameters
            .iter()
            .enumerate()
            .map(|(index, parameter)| (parameter.value.clone(), TypeParameterId(index as u32)))
            .collect::<Vec<_>>();
        let mut seen = BTreeSet::new();
        for (member, decorator) in checks {
            if !decorator.value.configured || decorator.value.arguments.len() != 1 {
                return Err(reject(
                    "@check requires exactly one check function",
                    decorator.location,
                ));
            }
            let (key, input) = match (declared.body.as_ref(), member) {
                (TypeDescriptor::Struct(_), None) => (
                    PropertyKey::Construction {
                        constructor,
                        variant: None,
                    },
                    unchecked_descriptor(target.clone()),
                ),
                (TypeDescriptor::Newtype(payload), None) => (
                    PropertyKey::Construction {
                        constructor,
                        variant: None,
                    },
                    payload.as_ref().clone(),
                ),
                (TypeDescriptor::Enum(variants), Some((index, member))) => {
                    let name = &member.value.name.as_ref().expect("named variant").value;
                    let payload =
                        variants
                            .get(name)
                            .and_then(Option::as_deref)
                            .ok_or_else(|| {
                                reject("unit variants do not accept @check", decorator.location)
                            })?;
                    (
                        PropertyKey::Construction {
                            constructor,
                            variant: Some(index),
                        },
                        payload.clone(),
                    )
                }
                _ => {
                    return Err(reject(
                        "@check is supported on structs, newtypes and payload variants",
                        decorator.location,
                    ));
                }
            };
            if !seen.insert(key) {
                return Err(reject(
                    "duplicate @check on the same construction boundary",
                    decorator.location,
                ));
            }
            let contract = TypeDescriptor::Function {
                parameters: vec![input],
                result: Box::new(result_descriptor(
                    TypeDescriptor::Tuple(Vec::new()),
                    TypeDescriptor::Opaque(crate::core::blame_native_type()),
                )),
            };
            let expression = &decorator.value.arguments[0];
            let plan = solve_tool_expression_types(
                expression,
                Some(&contract),
                query.clone(),
                sources,
                context,
            )
            .and_then(|evidence| prepare_tool_execution(expression, evidence, sources, &mut context.types))
            .map_err(|message| {
                reject(
                    &format!("invalid @check function: {message}"),
                    expression.location,
                )
            })?;
            plans.push(PreparedConstructionCheck {
                key,
                parameters: parameters.clone(),
                plan,
            });
        }
    }
    Ok(plans)
}

fn evaluate_construction_checks(
    source_name: &str,
    plans: &[PreparedConstructionCheck],
    tool_values: &BTreeMap<String, Val>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
    complete: bool,
) -> Result<(), FrontendError> {
    if evaluator.construction_checks_complete {
        return Ok(());
    }
    let mut publication = Vec::new();
    let mut pending = false;
    for check in plans {
        if !evaluator
            .registered_construction_checks
            .contains(&check.key)
        {
            evaluator.stage_property(
                check.key,
                Val::unknown(crate::heap::DecodedValue::BuiltinAtom(
                    crate::BuiltinAtom::None,
                )),
            )?;
        }
    }
    for check in plans {
        if evaluator
            .registered_construction_checks
            .contains(&check.key)
        {
            continue;
        }
        let mut bindings = ScopedToolBindings::new(tool_values);
        for (name, parameter) in &check.parameters {
            bindings.insert(
                name.clone(),
                evaluator.descriptor(&TypeDescriptor::Bound(*parameter))?,
            );
        }
        let value = match evaluate_prepared_tool_expression(
            source_name,
            &bindings,
            &check.plan,
            account,
            sources,
            evaluator,
            false,
        ) {
            Ok(value) => value,
            Err(_) if !complete => {
                pending = true;
                continue;
            }
            Err(error) => return Err(error),
        };
        evaluator.stage_property(check.key, value)?;
        evaluator.registered_construction_checks.insert(check.key);
        publication.push((check.key, value));
    }
    evaluator.publish_type_properties(None, &publication)?;
    evaluator.construction_checks_complete = !pending;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prepare_construction_dependencies(
    source_name: &str,
    program: &Program,
    hir: &HirProgram,
    plans: &mut [ToolBindingTask<'_>],
    checks: &[PreparedConstructionCheck],
    tool_values: &mut BTreeMap<String, Val>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<(), FrontendError> {
    evaluate_construction_checks(
        source_name,
        checks,
        tool_values,
        account,
        sources,
        evaluator,
        false,
    )?;
    if evaluator.construction_checks_complete {
        return Ok(());
    }
    let mut frontier = Vec::new();
    for binding in &program.value.body.value.bindings {
        let members = declared_member_fields(binding).into_iter().flatten();
        for decorator in binding
            .value
            .decorators
            .iter()
            .chain(members.flat_map(|member| member.value.decorators.iter()))
            .filter(|decorator| intrinsic_check_marker(decorator))
        {
            for argument in &decorator.value.arguments {
                for root in hir.expression_ids_at(argument.location) {
                    frontier.extend(expression_dependencies(hir, root));
                }
            }
        }
    }
    let mut dependencies = BTreeSet::new();
    while let Some(definition) = frontier.pop() {
        if dependencies.insert(definition)
            && let Some(root) = hir
                .definition(definition)
                .and_then(|definition| definition.value)
        {
            frontier.extend(expression_dependencies(hir, root));
        }
    }
    let names = dependencies
        .into_iter()
        .filter_map(|definition| hir.definition(definition))
        .filter(|definition| definition.top_level)
        .map(|definition| definition.name.as_str())
        .collect::<BTreeSet<_>>();
    for binding in &program.value.body.value.bindings {
        if names.contains(binding.value.name.value.as_str())
            && !tool_values.contains_key(&binding.value.name.value)
            && matches!(binding.value.kind, BindingKind::Def | BindingKind::Let)
            && !plans
                .iter()
                .any(|task| task.binding.value.name.location == binding.value.name.location)
        {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    "construction-check dependency has no static execution plan",
                    binding.location,
                ),
            ));
        }
    }
    loop {
        evaluate_construction_checks(
            source_name,
            checks,
            tool_values,
            account,
            sources,
            evaluator,
            false,
        )?;
        let mut progressed = false;
        for task in plans.iter_mut() {
            let binding = task.binding;
            let name = &binding.value.name.value;
            if !names.contains(name.as_str())
                || tool_values.contains_key(name)
                || !matches!(binding.value.kind, BindingKind::Def | BindingKind::Let)
            {
                continue;
            }
            if let Ok(value) = task.execute(source_name, tool_values, account, sources, evaluator) {
                tool_values.insert(name.clone(), value);
                progressed = true;
            }
        }
        if !progressed {
            return Ok(());
        }
    }
}
