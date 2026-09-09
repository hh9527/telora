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

#[allow(clippy::too_many_arguments)]
fn evaluate_construction_checks(
    source_name: &str,
    program: &Program,
    tool_values: &BTreeMap<String, Val>,
    environment: &HashMap<String, TypeDescriptor>,
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
    for register_only in [true, false] {
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
                if !complete {
                    pending = true;
                    continue;
                }
                return Err(reject(
                    "@check requires a nominal struct or payload variant",
                    binding.location,
                ));
            };
            let TypeDescriptor::Declared(declared) = target else {
                if !complete {
                    pending = true;
                    continue;
                }
                return Err(reject(
                    "@check requires a nominal struct or payload variant",
                    binding.location,
                ));
            };
            let constructor = declared.id.constructor();
            let mut bindings = ScopedToolBindings::new(tool_values);
            for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
                bindings.insert(
                    parameter.value.clone(),
                    evaluator.descriptor(&TypeDescriptor::Bound(TypeParameterId(index as u32)))?,
                );
            }
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
                        if !complete {
                            pending = true;
                            continue;
                        }
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
                if evaluator.registered_construction_checks.contains(&key) {
                    continue;
                }
                if register_only {
                    evaluator.stage_property(
                        key,
                        Val::unknown(crate::heap::DecodedValue::BuiltinAtom(
                            crate::BuiltinAtom::None,
                        )),
                    )?;
                    continue;
                }
                let contract = TypeDescriptor::Function {
                    parameters: vec![input],
                    result: Box::new(result_descriptor(
                        TypeDescriptor::Tuple(Vec::new()),
                        TypeDescriptor::Opaque(crate::core::blame_native_type()),
                    )),
                };
                let expression = &decorator.value.arguments[0];
                let evidence = match infer_tool_expression_evidence(
                    source_name,
                    expression,
                    &bindings,
                    Some(&contract),
                    account,
                    sources,
                    evaluator,
                ) {
                    Ok(evidence) => evidence,
                    Err(_) if !complete => {
                        pending = true;
                        continue;
                    }
                    Err(message) => {
                        return Err(reject(
                            &format!("invalid @check function: {message}"),
                            expression.location,
                        ));
                    }
                };
                let value = match evaluate_prepared_tool_expression(
                    source_name,
                    expression,
                    &bindings,
                    evidence,
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
                evaluator.stage_property(key, value)?;
                evaluator.registered_construction_checks.insert(key);
                publication.push((key, value));
            }
        }
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
    tool_values: &mut BTreeMap<String, Val>,
    environment: &HashMap<String, TypeDescriptor>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<(), FrontendError> {
    evaluate_construction_checks(
        source_name,
        program,
        tool_values,
        environment,
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
    loop {
        evaluate_construction_checks(
            source_name,
            program,
            tool_values,
            environment,
            account,
            sources,
            evaluator,
            false,
        )?;
        let mut progressed = false;
        for binding in &program.value.body.value.bindings {
            let name = &binding.value.name.value;
            if !names.contains(name.as_str())
                || tool_values.contains_key(name)
                || !matches!(binding.value.kind, BindingKind::Def | BindingKind::Let)
            {
                continue;
            }
            let expression = &binding.value.value;
            let annotation =
                if !environment.contains_key(name) && binding.value.type_parameters.is_empty() {
                    binding.value.annotation.as_ref().and_then(|annotation| {
                        evaluate_typed_tool_expression_silent(
                            source_name,
                            annotation,
                            tool_values,
                            &HashMap::new(),
                            Some(&TypeDescriptor::Type),
                            account,
                            sources,
                            evaluator,
                        )
                        .ok()
                        .and_then(|metadata| {
                            evaluator
                                .decode_type(metadata, "check dependency contract")
                                .ok()
                        })
                    })
                } else {
                    None
                };
            let expected = environment.get(name).or(annotation.as_ref());
            let Ok(evidence) = infer_tool_expression_evidence(
                source_name,
                expression,
                tool_values,
                expected,
                account,
                sources,
                evaluator,
            ) else {
                continue;
            };
            if let Ok(value) = evaluate_prepared_tool_expression(
                source_name,
                expression,
                tool_values,
                evidence,
                account,
                sources,
                evaluator,
                false,
            ) {
                tool_values.insert(name.clone(), value);
                progressed = true;
            }
        }
        if !progressed {
            return Ok(());
        }
    }
}
