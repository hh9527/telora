fn imported_static_descriptor(
    value: ValueRef<'_>,
    interface: Option<&ModuleInterface>,
    local: &str,
) -> TypeDescriptor {
    let Some(interface) = interface.filter(|interface| !interface.exports.is_empty()) else {
        return infer_value_ref(value);
    };
    if let Some(scheme) = interface.exports.get(local) {
        return erase_type_variables(&scheme.body);
    }
    TypeDescriptor::Struct(
        interface
            .exports
            .iter()
            .map(|(name, scheme)| (name.clone(), erase_type_variables(&scheme.body)))
            .collect(),
    )
}

fn validate_interpreter_contract(
    type_parameters: &[crate::ast::Identifier],
    contract: Option<&TypeDescriptor>,
) -> Result<(), String> {
    if contract.is_none() {
        return Err(
            "interpreter requires an explicit for(A, ...) Fn(TypeOf(A), ...) -> Fn(...) -> R definition contract"
                .into(),
        );
    }
    if type_parameters.is_empty() {
        return Err("interpreter requires at least one quantified type parameter".into());
    }
    let Some(TypeDescriptor::Function {
        parameters: outer_parameters,
        result: outer_result,
    }) = contract
    else {
        return Err(
            "interpreter contract must return an inner Function from explicit TypeOf witnesses"
                .into(),
        );
    };

    let mut witnesses = HashMap::new();
    for (index, witness) in outer_parameters.iter().enumerate() {
        let TypeDescriptor::TypeOf(parameter) = witness else {
            return Err(format!(
                "interpreter witness parameter {} must have type TypeOf(A)",
                index + 1
            ));
        };
        let TypeDescriptor::Bound(parameter) = parameter.as_ref() else {
            return Err(format!(
                "interpreter witness parameter {} must name a quantified type parameter",
                index + 1
            ));
        };
        let Some(name) = type_parameters.get(parameter.0 as usize) else {
            return Err("interpreter witness refers to an unknown type parameter".into());
        };
        if witnesses.insert(*parameter, index).is_some() {
            return Err(format!(
                "interpreter type parameter {} has more than one TypeOf witness",
                name.value
            ));
        }
    }
    for (index, parameter) in type_parameters.iter().enumerate() {
        if !witnesses.contains_key(&TypeParameterId(index as u32)) {
            return Err(format!(
                "interpreter type parameter {} has no TypeOf witness",
                parameter.value
            ));
        }
    }

    let TypeDescriptor::Function {
        parameters: inner_parameters,
        result,
    } = outer_result.as_ref()
    else {
        return Err("interpreter TypeOf witnesses must return an inner Function".into());
    };
    let interpreted = witnesses.keys().copied().collect::<HashSet<_>>();
    for (index, parameter) in inner_parameters.iter().enumerate() {
        if let TypeDescriptor::Bound(bound) = parameter
            && interpreted.contains(bound)
        {
            continue;
        }
        let mut mentioned = Vec::new();
        collect_bound_parameters(parameter, &mut mentioned);
        if let Some(bound) = mentioned
            .into_iter()
            .find(|bound| interpreted.contains(bound))
        {
            let name = &type_parameters[bound.0 as usize].value;
            return Err(format!(
                "interpreter inner parameter {} contains type parameter {}; only a direct {} parameter can be lifted",
                index + 1,
                name,
                name
            ));
        }
    }
    let mut result_parameters = Vec::new();
    collect_bound_parameters(result, &mut result_parameters);
    if let Some(bound) = result_parameters
        .into_iter()
        .find(|bound| interpreted.contains(bound))
    {
        return Err(format!(
            "interpreter result contains type parameter {}; lifted interpreters cannot return interpreted values",
            type_parameters[bound.0 as usize].value
        ));
    }
    Ok(())
}

pub(crate) fn infer_value_ref(value: ValueRef<'_>) -> TypeDescriptor {
    infer_value_ref_with(value, &mut HashSet::new())
}

fn infer_value_ref_with(
    value: ValueRef<'_>,
    visiting_type_slots: &mut HashSet<Handle>,
) -> TypeDescriptor {
    if let Some(handle) = value.hidden_type_slot_handle() {
        if !visiting_type_slots.insert(handle) {
            return TypeDescriptor::Any;
        }
        let inferred = value
            .resolve_hidden_type_slot()
            .map(|resolved| infer_value_ref_with(resolved, visiting_type_slots))
            .unwrap_or(TypeDescriptor::Any);
        visiting_type_slots.remove(&handle);
        return inferred;
    }
    if let Some((owner, payload)) = value.declared_value_parts() {
        return decode_type_ref(owner, "declared value owner")
            .unwrap_or_else(|_| infer_value_ref_with(payload, visiting_type_slots));
    }
    match value.kind() {
        ValueKind::Int => TypeDescriptor::Int,
        ValueKind::Float => TypeDescriptor::Float,
        ValueKind::String => TypeDescriptor::String,
        ValueKind::Bytes => TypeDescriptor::Bytes,
        ValueKind::Type => TypeDescriptor::TypeOf(Box::new(
            decode_type_ref(value, "Type").unwrap_or(TypeDescriptor::Any),
        )),
        ValueKind::Opaque => value
            .opaque_native_type()
            .cloned()
            .map(TypeDescriptor::Opaque)
            .unwrap_or(TypeDescriptor::Any),
        ValueKind::Atom => value
            .as_atom()
            .map(|atom| TypeDescriptor::Atom(atom_from_name(atom.as_str())))
            .unwrap_or(TypeDescriptor::Any),
        ValueKind::Array => {
            let items = (0..value.sequence_len().unwrap_or_default())
                .filter_map(|index| value.sequence_get(index))
                .map(|item| infer_value_ref_with(item, visiting_type_slots))
                .collect();
            TypeDescriptor::Array(Box::new(common_type(items).unwrap_or(TypeDescriptor::Any)))
        }
        ValueKind::Tagged => value
            .tagged_parts()
            .and_then(|(tag, payload)| {
                Some(TypeDescriptor::Tagged {
                    tag: atom_from_name(tag.as_atom()?.as_str()),
                    payload: Box::new(infer_value_ref_with(payload, visiting_type_slots)),
                })
            })
            .unwrap_or(TypeDescriptor::Any),
        ValueKind::Tuple => TypeDescriptor::Tuple(
            (0..value.sequence_len().unwrap_or_default())
                .filter_map(|index| value.sequence_get(index))
                .map(|item| infer_value_ref_with(item, visiting_type_slots))
                .collect(),
        ),
        ValueKind::Dict => TypeDescriptor::Struct(
            value
                .dict_fields()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|name| {
                    value.dict_get(name).map(|field| {
                        (
                            name.to_owned(),
                            infer_value_ref_with(field, visiting_type_slots),
                        )
                    })
                })
                .collect(),
        ),
        ValueKind::Func => TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Any; value.function_arity().unwrap_or_default()],
            result: Box::new(TypeDescriptor::Any),
        },
        ValueKind::Dyn => TypeDescriptor::Dyn,
        ValueKind::Module => TypeDescriptor::Struct(
            value
                .module_fields()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|name| {
                    value.module_get(name).map(|field| {
                        (
                            name.to_owned(),
                            infer_value_ref_with(field, visiting_type_slots),
                        )
                    })
                })
                .collect(),
        ),
    }
}

fn declare_metadata_value(
    source_name: &str,
    module_id: crate::ModuleId,
    binding: &Binding,
    slots: &HashMap<crate::Location, u32>,
    value: Val,
    evaluator: &mut ToolEvaluator,
) -> Result<Val, FrontendError> {
    if binding.value.declared_initializer.is_none() {
        return Ok(value);
    }
    validate_declared_metadata(source_name, binding, value, evaluator)?;
    let slot = slots
        .get(&binding.value.name.location)
        .copied()
        .expect("direct declared initializer has a declaration slot");
    evaluator
        .work
        .declare_type(value, module_id, slot, binding.value.name.value.as_str())
        .map_err(|error| {
            frontend_error(
                source_name,
                format!("declared type construction failed: {error}"),
            )
        })
}

fn validate_declared_metadata(
    source_name: &str,
    binding: &Binding,
    value: Val,
    evaluator: &ToolEvaluator,
) -> Result<(), FrontendError> {
    let kind = binding
        .value
        .declared_initializer
        .expect("declared metadata validation requires a declared initializer");
    let mut graph = TypeGraph::default();
    let root = graph
        .decode_persistent(
            ValueRef::work(value, &evaluator.work, evaluator.main),
            "Type",
            &mut HashMap::new(),
        )
        .map_err(|message| {
            frontend_error(
                source_name,
                format!(
                    "declared type {} produced invalid metadata: {message}",
                    binding.value.name.value
                ),
            )
        })?;
    let valid = graph.root_model_kind(root) == Some(kind);
    if !valid {
        return Err(frontend_error(
            source_name,
            format!(
                "declared type {} initializer changed its root model kind",
                binding.value.name.value
            ),
        ));
    }
    Ok(())
}

fn is_declared_literal_construction(
    expression: &Expr,
    _actual: &TypeDescriptor,
    expected: &TypeDescriptor,
) -> bool {
    match (expected, &expression.value) {
        (TypeDescriptor::Declared(declared), _) => {
            declared_body_accepts_expression(&declared.body, expression)
        }
        (TypeDescriptor::Array(item), ExprKind::Array(items))
            if matches!(item.as_ref(), TypeDescriptor::Declared(_)) =>
        {
            items.iter().all(|item_expression| {
                !matches!(item_expression.value, ExprKind::Spread(_))
                    && is_declared_literal_construction(item_expression, &TypeDescriptor::Any, item)
            })
        }
        _ => false,
    }
}

fn declared_body_accepts_expression(body: &TypeDescriptor, expression: &Expr) -> bool {
    match (body, &expression.value) {
        (TypeDescriptor::Struct(_), ExprKind::Dict(_))
        | (TypeDescriptor::Enum(_), ExprKind::Atom(_)) => true,
        (TypeDescriptor::Enum(_), ExprKind::Call { callee, .. }) => {
            matches!(callee.value, ExprKind::Atom(_))
        }
        _ => false,
    }
}

fn evaluate_tool_expression(
    source_name: &str,
    expression: &Expr,
    bindings: &BTreeMap<String, Val>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
) -> Result<Val, FrontendError> {
    evaluate_tool_expression_with_debug(
        source_name,
        expression,
        bindings,
        None,
        account,
        sources,
        evaluator,
        true,
    )
}

fn evaluate_typed_tool_expression_silent(
    source_name: &str,
    expression: &Expr,
    bindings: &BTreeMap<String, Val>,
    expression_descriptors: &HashMap<crate::Location, TypeDescriptor>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
) -> Result<Val, FrontendError> {
    evaluate_tool_expression_with_debug(
        source_name,
        expression,
        bindings,
        Some(expression_descriptors),
        account,
        sources,
        evaluator,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_tool_expression_with_debug(
    source_name: &str,
    expression: &Expr,
    bindings: &BTreeMap<String, Val>,
    expression_descriptors: Option<&HashMap<crate::Location, TypeDescriptor>>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator,
    observed: bool,
) -> Result<Val, FrontendError> {
    let mut bindings = bindings.clone();
    let mut declared_value_owners = HashMap::new();
    if let Some(expression_descriptors) = expression_descriptors {
        for (location, descriptor) in
            expression_descriptors
                .iter()
                .filter(|(location, descriptor)| {
                    expression.location.start <= location.start
                        && location.end <= expression.location.end
                        && matches!(descriptor, TypeDescriptor::Declared(_))
                        && !type_identity_is_symbolic(descriptor)
                })
        {
            let key = crate::compiler::declared_owner_link_key(*location);
            bindings.insert(key.clone(), evaluator.descriptor(descriptor)?);
            declared_value_owners.insert(*location, key);
        }
    }
    let function = compile_expression_with_external_bindings(
        source_name,
        "<tool-stage>",
        expression,
        bindings.keys().cloned(),
        declared_value_owners,
        sources.get(expression.location.source),
    )?;
    let externals = bindings
        .iter()
        .map(|(name, value)| (name.clone(), *value))
        .collect::<HashMap<_, _>>();
    let work = std::mem::replace(&mut evaluator.work, Heap::work_for(evaluator.main));
    let vm = if observed {
        &mut evaluator.observed_vm
    } else {
        &mut evaluator.silent_vm
    };
    let root =
        match vm.execute_in_existing_work(evaluator.main, &externals, &function, work, account) {
            Ok((work, root)) => {
                evaluator.work = work;
                root
            }
            Err((work, error)) => {
                evaluator.work = work;
                return Err(frontend_error(
                    source_name,
                    format!(
                        "tool-stage evaluation failed: {}",
                        error.with_sources(sources)
                    ),
                ));
            }
        };
    Ok(root)
}

