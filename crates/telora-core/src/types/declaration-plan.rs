// Declaration solving records graph roots; this pass only materializes them.
enum DeclarationPlan<'a> {
    Concrete { binding: &'a Binding, root: AnalysisTypeId },
    Family {
        binding: &'a Binding,
        body: AnalysisTypeId,
        parameters: Vec<TypeParameter>,
        constructor: Option<NominalTypeConstructor>,
        rebuild_at_runtime: bool,
    },
    RecursiveFamily {
        binding: &'a Binding,
        solved: SolvedRecursiveType,
        parameters: Vec<TypeParameter>,
        rebuild_at_runtime: bool,
    },
    RecursiveGroup { bindings: Vec<&'a Binding>, roots: Vec<SolvedRecursiveType> },
}

#[allow(clippy::too_many_arguments)]
fn materialize_declarations(
    plans: Vec<DeclarationPlan<'_>>,
    graph: &TypeGraph,
    module_id: crate::ModuleId,
    slots: &HashMap<crate::Location, u32>,
    source_name: &str,
    values: &mut BTreeMap<String, Val>,
    families: &mut BTreeMap<String, TypeFamilyTemplate>,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<(), FrontendError> {
    for plan in &plans {
        let mut reserve = |binding: &Binding| -> Result<(), FrontendError> {
            let name = &binding.value.name.value;
            let placeholder = evaluator.descriptor(&TypeDescriptor::Named(name.clone()))?;
            values.insert(name.clone(), placeholder);
            Ok(())
        };
        match plan {
            DeclarationPlan::Concrete { binding, .. } | DeclarationPlan::Family { binding, .. }
            | DeclarationPlan::RecursiveFamily { binding, .. } => reserve(binding)?,
            DeclarationPlan::RecursiveGroup { bindings, .. } => {
                for binding in bindings { reserve(binding)?; }
            }
        }
    }
    for plan in plans {
        let (binding, family_value, family) = match plan {
            DeclarationPlan::Concrete { binding, root } => {
                let value = materialize_type_body(root, graph, binding, source_name, values, evaluator)?.0;
                values.insert(binding.value.name.value.clone(), value);
                continue;
            }
            DeclarationPlan::Family { binding, body, parameters, constructor, rebuild_at_runtime } => {
                let value = materialize_type_body(body, graph, binding, source_name, values, evaluator)?.0;
                let (value, template, root) = evaluator.create_type_family(value, parameters.len(), constructor.as_ref())?;
                (binding, value, TypeFamilyTemplate { template, root, constructor, rebuild_at_runtime })
            }
            DeclarationPlan::RecursiveFamily { binding, solved, parameters, rebuild_at_runtime } => {
                let built = build_recursive_type_family(source_name, module_id, slots[&binding.value.name.location],
                    binding, values, &parameters, rebuild_at_runtime, evaluator, (graph, &solved))?;
                (binding, built.family_value, built.family)
            }
            DeclarationPlan::RecursiveGroup { bindings, roots } => {
                let mut references = Vec::with_capacity(bindings.len());
                for binding in &bindings {
                    let name = &binding.value.name.value;
                    let placeholder = values[name];
                    let reference = evaluator.work.reserve_type_ref(module_id,
                        slots[&binding.value.name.location], name.as_str(), placeholder)
                        .map_err(|error| frontend_error(source_name, format!("declared type reservation failed: {error}")))?;
                    values.insert(name.clone(), reference);
                    references.push(reference);
                }
                let bodies = bindings.iter().zip(&roots).map(|(binding, solved)| {
                    materialize_type_body(solved.body, graph, binding, source_name, values, evaluator)
                        .map(|(value, _)| value)
                }).collect::<Result<Vec<_>, _>>()?;
                for (reference, body) in references.into_iter().zip(bodies) {
                    evaluator.work.seal_type_ref(reference, body)
                        .map_err(|error| frontend_error(source_name, error.to_string()))?;
                }
                continue;
            }
        };
        let name = binding.value.name.value.clone();
        values.insert(name.clone(), family_value);
        families.insert(name, family);
    }
    Ok(())
}
