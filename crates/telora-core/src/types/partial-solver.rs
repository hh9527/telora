#[derive(Default)]
struct PartialTypeInputs {
    module_id: Option<crate::ModuleId>,
    names: BTreeSet<String>,
    imported_types: HashMap<String, TypeDescriptor>,
    imported_values: BTreeMap<String, TypeDescriptor>,
    interfaces: BTreeMap<String, ModuleInterface>,
}

#[cfg(test)]
mod partial_solver_tests {
    use super::*;

    #[test]
    fn solves_recursive_families_and_reports_unknowns_without_runtime_resources() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("pure-type-solver",
            "type Tree(T) = struct {value: T, children: Array(Tree(T))}; type Strings = Tree(String); type Missing = Absent; 0");
        let parsed = parse_registered(&sources, source);
        let partial = solve_partial_types(&sources, source, &parsed.recovered,
            parsed.diagnostics, PartialTypeInputs::default(), PartialAnalysisControl {
                unavailable_imports: &HashSet::new(), external_schemes: &BTreeMap::new(),
                external_interfaces: &BTreeMap::new(), query: None,
            });
        let fact = |name: &str| {
            let definition = partial.hir.definitions().iter().find(|definition| definition.name == name).unwrap();
            &partial.definition_facts[&definition.id]
        };
        assert_eq!(fact("Tree").state, FactState::Known);
        assert_eq!(fact("Strings").state, FactState::Known);
        assert_eq!(fact("Missing").state, FactState::Unknown(UnknownReason::UnresolvedName));
        assert_eq!(partial.diagnostics.len(), 1);
        assert_eq!(partial.diagnostics[0].message, "unknown binding \"Absent\"");
    }
}

// This solver receives descriptions only: no VM, evaluator or runtime heap.
fn solve_partial_types(
    sources: &SourceDatabase, source_id: crate::SourceId,
    recovered: &crate::parser::RecoveredProgram, initial_diagnostics: Vec<Diagnostic>,
    inputs: PartialTypeInputs, control: PartialAnalysisControl<'_>,
) -> PartialAnalysis {
    let PartialTypeInputs { module_id, names, imported_types, imported_values, interfaces } = inputs;
    let external_names = names.iter().map(String::as_str).collect();
    let source_name = sources.get(source_id).name.to_string();
    let module_id = module_id.unwrap_or(crate::ModuleId::ANONYMOUS);
    let prelude = BootstrapPrelude::new();
    let hir = HirProgram::resolve_recovered_with_member_constructors(
        recovered,
        prelude
            .types
            .keys()
            .filter(|name| !names.contains(*name))
            .chain(names.iter())
            .cloned()
            .collect::<Vec<_>>(),
        control.external_interfaces.iter().filter(|(name, interface)|
            interface.value_binding.as_deref() == Some(name.as_str())
                && interface.member_constructors.contains_key(*name))
            .map(|(name, _)| name.clone()).collect(),
    );
    let bindings = type_definition_bindings(&hir, &recovered.bindings);
    let declared_initializer_slots = recovered
        .bindings
        .iter()
        .filter(|binding| binding.value.declared_initializer.is_some())
        .enumerate()
        .map(|(slot, binding)| {
            let slot = u32::try_from(slot)
                .expect("type constructor count exceeds u32")
                .checked_add(crate::FIRST_DYNAMIC_MODULE_LOCAL)
                .expect("type constructor slot exceeds u32");
            (binding.value.name.location, slot)
        })
        .collect::<HashMap<_, _>>();
    let type_definitions = bindings.keys().copied().collect::<HashSet<_>>();
    let import_definitions = hir
        .definitions()
        .iter()
        .filter(|definition| {
            definition.top_level
                && definition.kind == HirDefinitionKind::Import
                && control.unavailable_imports.contains(&definition.name)
        })
        .map(|definition| definition.id)
        .collect::<HashSet<_>>();
    let mut unavailable_dependencies = BTreeMap::new();
    for definition in bindings.keys() {
        if let Some(import) = definition_dependencies(&hir, *definition)
            .into_iter()
            .find(|dependency| import_definitions.contains(dependency))
        {
            unavailable_dependencies.insert(*definition, import);
        }
    }
    let dependencies = type_dependency_graph(&hir, &type_definitions);
    let dependency_plan = TypeDependencyPlan::new(&dependencies);
    let ordered_definitions = dependency_plan.order(&type_definitions.iter().copied().collect())
        .into_iter().flatten().collect::<Vec<_>>();
    let mut diagnostics = initial_diagnostics;
    let mut facts: BTreeMap<HirDefinitionId, SemanticFact<AnalysisTypeId>> = BTreeMap::new();
    let mut boundary = TypeBoundary::new(&hir, control.external_interfaces);
    boundary.external_data.extend(names.iter().cloned());
    boundary.bindings(&recovered.bindings);
    for diagnostic in boundary.diagnostics {
        let id = if let Some(index) = diagnostics.iter().position(|existing| existing == &diagnostic) {
            DiagnosticId::from_index(index)
        } else {
            let id = DiagnosticId::from_index(diagnostics.len());
            diagnostics.push(diagnostic.clone());
            id
        };
        if let Some(location) = diagnostic.labels.first().map(|label| label.location) {
            for (definition, binding) in &bindings {
                if binding.location.source == location.source
                    && binding.location.start <= location.start && location.end <= binding.location.end
                {
                    let mut fact = SemanticFact::conflicted(None, Conflict::IncompatibleContract);
                    fact.diagnostics.push(id);
                    facts.insert(*definition, fact);
                    break;
                }
            }
        }
    }
    let mut definition_schemes = BTreeMap::new();
    for (definition, import) in unavailable_dependencies {
        let cause = FactIdentity::HirDefinition(import);
        let mut fact = SemanticFact::unknown(UnknownReason::BlockedBy(cause));
        fact.causes.push(cause);
        facts.insert(definition, fact);
    }
    let mut types = TypeGraph::default();
    let mut environment = prelude.types.clone();
    let mut schemes = prelude.schemes.clone();
    schemes.extend(control.external_schemes.iter().map(|(name, scheme)| (name.clone(), scheme.clone())));
    environment.extend(imported_types);
    let mut static_families = static_type_families(&mut types, &interfaces);
    while facts.len() < bindings.len() {
        let mut progressed = false;
        for definition in &ordered_definitions {
            if control.query.is_some_and(|query| query.check().is_err()) {
                for definition in bindings.keys() {
                    facts.entry(*definition).or_insert_with(||
                        SemanticFact::incomputable(None, IncomputableReason::Cancelled));
                }
                break;
            }
            let node = dependency_plan.node(*definition);
            if facts.contains_key(&node.definition) {
                continue;
            }
            let blocked = node.dependencies.iter().find(|dependency| {
                facts
                    .get(*dependency)
                    .is_some_and(|fact| fact.state != FactState::Known)
            });
            if let Some(dependency) = blocked {
                let cause = FactIdentity::HirDefinition(*dependency);
                let mut fact = SemanticFact::unknown(UnknownReason::BlockedBy(cause));
                fact.causes.push(cause);
                facts.insert(node.definition, fact);
                progressed = true;
                continue;
            }
            if node
                .dependencies
                .iter()
                .any(|dependency| !facts.contains_key(dependency))
            {
                continue;
            }

            let binding = bindings[&node.definition];
            let mut parameters = Vec::new();
            let mut parameter_names = HashSet::new();
            for (index, parameter) in binding.value.type_parameters.iter().enumerate() {
                if !parameter_names.insert(parameter.value.as_str()) {
                    let diagnostic = DiagnosticId::from_index(diagnostics.len());
                    diagnostics.push(Diagnostic::error(
                        format!("duplicate type parameter {:?}", parameter.value),
                        parameter.location,
                    ));
                    let mut fact = SemanticFact::conflicted(None, Conflict::IncompatibleContract);
                    fact.diagnostics.push(diagnostic);
                    facts.insert(node.definition, fact);
                    break;
                }
                let Ok(index) = u32::try_from(index) else {
                    let diagnostic = DiagnosticId::from_index(diagnostics.len());
                    diagnostics.push(Diagnostic::error(
                        "type family has too many parameters",
                        parameter.location,
                    ));
                    let mut fact =
                        SemanticFact::incomputable(None, IncomputableReason::UnsupportedOperation);
                    fact.diagnostics.push(diagnostic);
                    facts.insert(node.definition, fact);
                    break;
                };
                let id = TypeParameterId(index);
                parameters.push(TypeParameter {
                    id,
                    name: parameter.value.clone(),
                    location: parameter.location,
                });
            }
            if facts.contains_key(&node.definition) {
                progressed = true;
                continue;
            }
            let conflict_start = types.elaboration_conflicts.len();
            let root = StaticContractScope { hir: &hir, environment: &environment,
                external_names: &external_names, interfaces: &interfaces,
                parameters: &parameters, families: &static_families,
            }.elaborate(&binding.value.value, &mut types);
            let Some(root) = root else {
                let fact = static_elaboration_conflict_fact(&types, conflict_start, &mut diagnostics)
                    .unwrap_or_else(|| SemanticFact::unknown(UnknownReason::BlockedBy(
                        FactIdentity::HirDefinition(node.definition))));
                facts.insert(node.definition, fact);
                progressed = true;
                continue;
            };
            let outcome = types.descriptor(root).map_err(|message| frontend_error(&source_name, message))
                .and_then(|body| {
                    let descriptor = if binding.value.declared_initializer.is_some() {
                        let arguments = parameters.iter()
                            .map(|parameter| TypeDescriptor::Bound(parameter.id))
                            .collect::<Vec<_>>();
                        TypeDescriptor::Declared(DeclaredTypeDescriptor {
                            id: crate::value::DeclaredTypeId::applied(module_id,
                                declared_initializer_slots[&binding.value.name.location], &arguments),
                            name: binding.value.name.value.clone(),
                            body: Arc::new(body.clone()),
                        })
                    } else { body.clone() };
                    Ok(descriptor)
                });
            match outcome {
                Ok(descriptor) => {
                    let definition_descriptor = if parameters.is_empty() {
                        let declared = types.intern_descriptor(&descriptor);
                        types
                            .names
                            .insert(binding.value.name.value.clone(), declared);
                        descriptor
                    } else {
                        let mut bounds = Vec::new();
                        collect_bound_parameters(&descriptor, &mut bounds);
                        if let Some(foreign) = bounds.iter().find(|bound| {
                            !parameters.iter().any(|parameter| parameter.id == **bound)
                        }) {
                            let diagnostic = DiagnosticId::from_index(diagnostics.len());
                            diagnostics.push(Diagnostic::error(
                                format!(
                                    "type family {} produced foreign bound parameter T{}",
                                    binding.value.name.value, foreign.0
                                ),
                                binding.value.value.location,
                            ));
                            let mut fact =
                                SemanticFact::conflicted(None, Conflict::IncompatibleContract);
                            fact.diagnostics.push(diagnostic);
                            facts.insert(node.definition, fact);
                            progressed = true;
                            continue;
                        }
                        let scheme = static_type_family_scheme(parameters, descriptor);
                        let projected = scheme.body.clone();
                        definition_schemes.insert(node.definition, scheme);
                        projected
                    };
                    let id = types.intern_descriptor(&definition_descriptor);
                    if let Some(family) = definition_schemes.get(&node.definition)
                        .and_then(|scheme| StaticTypeFamily::from_scheme(scheme, &mut types)) {
                        static_families.insert(binding.value.name.value.clone(), family);
                    }
                    let witness = if definition_schemes.contains_key(&node.definition) {
                        definition_descriptor.clone()
                    } else { TypeDescriptor::TypeOf(Box::new(definition_descriptor.clone())) };
                    environment.insert(binding.value.name.value.clone(), witness);
                    facts.insert(node.definition, SemanticFact::known(id));
                }
                Err(error) => {
                    let state = classify_partial_error(&error.message);
                    let diagnostic = DiagnosticId::from_index(diagnostics.len());
                    diagnostics.push(error.diagnostic.map_or_else(
                        || Diagnostic::error(error.message, binding.value.value.location),
                        |diagnostic| *diagnostic,
                    ));
                    let mut fact = match state {
                        FactState::Conflicted(conflict) => SemanticFact::conflicted(None, conflict),
                        FactState::Incomputable(reason) => SemanticFact::incomputable(None, reason),
                        FactState::Unknown(reason) => SemanticFact::unknown(reason),
                        FactState::Known => unreachable!("errors cannot produce known facts"),
                    };
                    fact.diagnostics.push(diagnostic);
                    facts.insert(node.definition, fact);
                }
            }
            progressed = true;
        }
        if progressed {
            continue;
        }

        let cyclic = dependencies
            .nodes
            .iter()
            .filter(|node| !facts.contains_key(&node.definition))
            .filter(|node| dependency_plan.is_cyclic(node.definition))
            .map(|node| node.definition)
            .collect::<Vec<_>>();
        let had_cycle = !cyclic.is_empty();
        let mut handled = HashSet::new();
        for root in cyclic {
            if !handled.insert(root) {
                continue;
            }
            let component = dependency_plan.component(root).iter().copied()
                .filter(|definition| !facts.contains_key(definition))
                .collect::<Vec<_>>();
            handled.extend(component.iter().copied());
            let recursive_nominal_family = component.len() == 1 && {
                let binding = bindings[&component[0]];
                !binding.value.type_parameters.is_empty()
                    && binding.value.declared_initializer.is_some()
            };
            if recursive_nominal_family {
                let definition = component[0];
                let binding = bindings[&definition];
                let parameters = match static_contract_parameters(binding, sources) {
                    Ok(parameters) => parameters,
                    Err(error) => {
                        let diagnostic = DiagnosticId::from_index(diagnostics.len());
                        diagnostics.push(error.diagnostic.map_or_else(
                            || Diagnostic::error(error.message, binding.value.value.location),
                            |diagnostic| *diagnostic,
                        ));
                        let mut fact = SemanticFact::conflicted(None, Conflict::IncompatibleContract);
                        fact.diagnostics.push(diagnostic);
                        facts.insert(definition, fact);
                        continue;
                    }
                };
                let conflict_start = types.elaboration_conflicts.len();
                let solved = elaborate_recursive_family(binding, module_id,
                    declared_initializer_slots[&binding.value.name.location], &parameters, &hir,
                    &environment, &external_names, &interfaces, &mut static_families, &mut types);
                let Some(solved) = solved else {
                    let fact = static_elaboration_conflict_fact(&types, conflict_start, &mut diagnostics)
                        .unwrap_or_else(|| SemanticFact::unknown(UnknownReason::BlockedBy(
                            FactIdentity::HirDefinition(definition))));
                    facts.insert(definition, fact);
                    continue;
                };
                let outcome = validate_declared_graph(&source_name, binding, &types, solved.body)
                    .and_then(|()| types.descriptor(solved.owner)
                        .map_err(|message| frontend_error(&source_name, message)))
                    .map(|body| static_type_family_scheme(parameters, body));
                match outcome {
                    Ok(scheme) => {
                        let descriptor = scheme.body.clone();
                        let id = types.intern_descriptor(&descriptor);
                        if let Some(family) = StaticTypeFamily::from_scheme(&scheme, &mut types) {
                            static_families.insert(binding.value.name.value.clone(), family);
                        }
                        environment.insert(binding.value.name.value.clone(), descriptor.clone());
                        definition_schemes.insert(definition, scheme);
                        facts.insert(definition, SemanticFact::known(id));
                    }
                    Err(error) => {
                        let diagnostic = DiagnosticId::from_index(diagnostics.len());
                        diagnostics.push(error.diagnostic.map_or_else(
                            || Diagnostic::error(error.message, binding.value.value.location),
                            |diagnostic| *diagnostic,
                        ));
                        let mut fact = SemanticFact::incomputable(
                            None,
                            IncomputableReason::UnsupportedOperation,
                        );
                        fact.diagnostics.push(diagnostic);
                        facts.insert(definition, fact);
                    }
                }
                continue;
            }
            let concrete_nominal = component.iter().all(|definition| {
                let binding = bindings[definition];
                binding.value.type_parameters.is_empty()
                    && binding.value.declared_initializer.is_some()
            });
            if concrete_nominal {
                let component_bindings = component.iter().map(|id| bindings[id]).collect::<Vec<_>>();
                let conflict_start = types.elaboration_conflicts.len();
                let solved = elaborate_recursive_bodies(&component_bindings, module_id,
                    &declared_initializer_slots, &mut environment, &hir,
                    &external_names, &interfaces, &static_families, &mut types);
                if let Some(solved) = solved {
                    for (definition, solved) in component.iter().zip(solved) {
                        let binding = bindings[definition];
                        let name = &binding.value.name.value;
                        facts.insert(*definition, SemanticFact::known(solved.body));
                        let descriptor = types.descriptor(solved.owner)
                            .expect("resolved recursive owner");
                        environment.insert(name.clone(), TypeDescriptor::TypeOf(Box::new(descriptor)));
                    }
                } else {
                    let conflict = static_elaboration_conflict_fact(&types, conflict_start, &mut diagnostics);
                    for definition in &component {
                        facts.entry(*definition).or_insert_with(|| {
                            conflict.clone().unwrap_or_else(|| SemanticFact::unknown(UnknownReason::BlockedBy(
                                FactIdentity::HirDefinition(root),
                            )))
                        });
                    }
                }
                continue;
            }
            for definition in component {
                let binding = bindings[&definition];
                let diagnostic = DiagnosticId::from_index(diagnostics.len());
                diagnostics.push(Diagnostic::error(
                    format!(
                        "recursive type component containing {:?} cannot be partially evaluated",
                        binding.value.name.value
                    ),
                    binding.value.name.location,
                ));
                let mut fact =
                    SemanticFact::incomputable(None, IncomputableReason::CyclicEvaluation);
                fact.diagnostics.push(diagnostic);
                facts.insert(definition, fact);
            }
        }
        if had_cycle {
            continue;
        }
        break;
    }
    // Convergence diagnostics belong to the unresolved root, not every dependent.
    for (definition, fact) in &mut facts {
        if fact.state != FactState::Unknown(UnknownReason::BlockedBy(
            FactIdentity::HirDefinition(*definition))) { continue; }
        let binding = bindings[definition];
        let unresolved = hir.references().iter().find(|reference| {
            reference.resolution.is_unresolved()
                && reference.location.source == binding.location.source
                && binding.location.start <= reference.location.start
                && reference.location.end <= binding.location.end
        });
        let diagnostic = if let Some(reference) = unresolved {
            fact.state = FactState::Unknown(UnknownReason::UnresolvedName);
            Diagnostic::error(format!("unknown binding {:?}", reference.name), reference.location)
        } else {
            Diagnostic::error("type remains unknown after static solving", binding.value.value.location)
        };
        fact.diagnostics.push(DiagnosticId::from_index(diagnostics.len()));
        diagnostics.push(diagnostic);
    }
    for definition in hir
        .definitions()
        .iter()
        .filter(|definition| definition.top_level && definition.kind == HirDefinitionKind::Import)
    {
        if let Some(scheme) = control.external_schemes.get(&definition.name) {
            definition_schemes.insert(definition.id, scheme.clone());
        }
        if facts.contains_key(&definition.id)
            || control.unavailable_imports.contains(&definition.name)
        {
            continue;
        }
        let Some(descriptor) = imported_values.get(&definition.name) else {
            continue;
        };
        let ty = types.intern_descriptor(descriptor);
        facts.insert(definition.id, SemanticFact::known(ty));
    }
    let mut indexed_diagnostics = diagnostics.into_iter().enumerate().collect::<Vec<_>>();
    indexed_diagnostics.sort_by_key(|(_, diagnostic)| {
        diagnostic
            .labels
            .first()
            .map_or(0, |label| label.location.start)
    });
    let mut remapped_diagnostics = vec![DiagnosticId::from_index(0); indexed_diagnostics.len()];
    for (new, (old, _)) in indexed_diagnostics.iter().enumerate() {
        remapped_diagnostics[*old] = DiagnosticId::from_index(new);
    }
    for fact in facts.values_mut() {
        for diagnostic in &mut fact.diagnostics {
            *diagnostic = remapped_diagnostics[diagnostic.index()];
        }
    }
    let diagnostics = indexed_diagnostics
        .into_iter()
        .map(|(_, diagnostic)| diagnostic)
        .collect();
    PartialAnalysis {
        hir,
        dependencies,
        definition_facts: facts,
        definition_schemes,
        diagnostics,
        types,
    }
}

fn static_elaboration_conflict_fact(
    graph: &TypeGraph, start: usize, diagnostics: &mut Vec<Diagnostic>,
) -> Option<SemanticFact<AnalysisTypeId>> {
    let conflicts = &graph.elaboration_conflicts[start..];
    if conflicts.is_empty() { return None; }
    let mut fact = SemanticFact::conflicted(None, Conflict::IncompatibleContract);
    for diagnostic in conflicts {
        fact.diagnostics.push(DiagnosticId::from_index(diagnostics.len()));
        diagnostics.push(diagnostic.clone());
    }
    Some(fact)
}
