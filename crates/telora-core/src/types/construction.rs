fn intrinsic_check_marker(decorator: &crate::ast::Decorator) -> bool {
    matches!(&decorator.value.callee.value,
        ExprKind::Variable(name) if name.value == "check")
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
) -> Result<(), FrontendError> {
    let mut publication = Vec::new();
    for binding in &program.value.body.value.bindings {
        let mut checks = binding.value.decorators.iter()
            .filter(|decorator| intrinsic_check_marker(decorator))
            .map(|decorator| (None, decorator)).collect::<Vec<_>>();
        if let Some(members) = declared_member_fields(binding) {
            let mut members = members.iter().collect::<Vec<_>>();
            members.sort_by_key(|member| member.value.name.as_ref().map(|name| &name.value));
            for (index, member) in members.into_iter().enumerate() {
                checks.extend(member.value.decorators.iter()
                    .filter(|decorator| intrinsic_check_marker(decorator))
                    .map(|decorator| (Some((index as u32, member)), decorator)));
            }
        }
        if checks.is_empty() { continue; }
        let reject = |message: &str, location| FrontendError::from_diagnostic(
            sources, Diagnostic::error(message, location),
        );
        let target = environment.get(&binding.value.name.value)
            .and_then(constructor_instance_type)
            .ok_or_else(|| reject("@check requires a nominal struct or payload variant", binding.location))?;
        let TypeDescriptor::Declared(declared) = target else {
            return Err(reject("@check requires a nominal struct or payload variant", binding.location));
        };
        let constructor = declared.id.constructor();
        let mut bindings = tool_values.clone();
        for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
            bindings.insert(parameter.value.clone(), evaluator.descriptor(
                &TypeDescriptor::Bound(TypeParameterId(index as u32)),
            )?);
        }
        let mut seen = BTreeSet::new();
        for (member, decorator) in checks {
            if !decorator.value.configured || decorator.value.arguments.len() != 1 {
                return Err(reject("@check requires exactly one check function", decorator.location));
            }
            let (key, input) = match (declared.body.as_ref(), member) {
                (TypeDescriptor::Struct(_), None) => (
                    PropertyKey::Construction { constructor, variant: None },
                    unchecked_descriptor(target.clone()),
                ),
                (TypeDescriptor::Newtype(payload), None) => (
                    PropertyKey::Construction { constructor, variant: None },
                    payload.as_ref().clone(),
                ),
                (TypeDescriptor::Enum(variants), Some((index, member))) => {
                    let name = &member.value.name.as_ref().expect("named variant").value;
                    let payload = variants.get(name).and_then(Option::as_deref)
                        .ok_or_else(|| reject("unit variants do not accept @check", decorator.location))?;
                    (PropertyKey::Construction { constructor, variant: Some(index) }, payload.clone())
                }
                _ => return Err(reject("@check is supported on structs, newtypes and payload variants", decorator.location)),
            };
            if !seen.insert(key) {
                return Err(reject("duplicate @check on the same construction boundary", decorator.location));
            }
            let contract = TypeDescriptor::Function {
                parameters: vec![input],
                result: Box::new(option_descriptor(TypeDescriptor::Opaque(crate::core::blame_native_type()))),
            };
            let expression = &decorator.value.arguments[0];
            let evidence = infer_tool_expression_evidence(source_name, expression, &bindings,
                Some(&contract), account, sources, evaluator)
                .map_err(|message| reject(&format!("invalid @check function: {message}"), expression.location))?;
            let value = evaluate_typed_tool_expression_silent(source_name, expression, &bindings,
                &evidence.descriptors, Some(&contract), account, sources, evaluator)?;
            evaluator.stage_property(key, value)?;
            publication.push((key, value));
        }
    }
    evaluator.publish_type_properties(None, &publication)
}
