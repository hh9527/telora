#[derive(Clone, Debug)]
pub struct Analysis {
    pub types: TypeGraph,
    pub declared_types: BTreeMap<String, AnalysisTypeId>,
    pub binding_types: BTreeMap<String, AnalysisTypeId>,
    pub trait_ids: BTreeMap<String, crate::TraitId>,
    pub trait_implementations: Vec<TraitImplementation>,
    pub result_type: AnalysisTypeId,
    pub hir: HirProgram,
    pub definition_types: BTreeMap<HirDefinitionId, AnalysisTypeId>,
    pub definition_schemes: BTreeMap<HirDefinitionId, TypeScheme>,
    pub expression_types: BTreeMap<HirExpressionId, AnalysisTypeId>,
    pub module_interface: ModuleInterface,
    pub explicit_exports: bool,
    pub(crate) propagation_families: HashMap<crate::Location, PropagationFamily>,
    pub(crate) not_families: HashMap<crate::Location, NotFamily>,
    pub(crate) trait_member_evidence: HashMap<crate::Location, ResolvedEvidence>,
    pub(crate) generic_call_evidence: HashMap<crate::Location, Vec<ResolvedEvidence>>,
    pub(crate) interpolation_evidence: HashMap<crate::Location, ResolvedEvidence>,
    pub(crate) generic_evidence_parameters: HashMap<crate::Location, Vec<String>>,
    pub(crate) generic_dictionary_factories: HashMap<crate::Location, Vec<String>>,
    pub(crate) runtime_roots: BTreeMap<String, PersistentValue>,
    pub(crate) external_bindings: HashSet<String>,
    pub(crate) dynamic_bindings: HashSet<String>,
    pub(crate) type_family_values: BTreeMap<String, TypeFamilyTemplate>,
    pub(crate) declared_value_owners: HashMap<crate::Location, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PropagationFamily {
    Option,
    Result,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NotFamily {
    Bool,
    Int,
    Dynamic,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticDependencyNode {
    pub definition: HirDefinitionId,
    pub dependencies: Vec<HirDefinitionId>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SemanticDependencyGraph {
    pub nodes: Vec<SemanticDependencyNode>,
}

#[derive(Clone, Debug)]
pub struct PartialAnalysis {
    pub hir: HirProgram,
    pub dependencies: SemanticDependencyGraph,
    pub definition_facts: BTreeMap<HirDefinitionId, SemanticFact<AnalysisTypeId>>,
    pub definition_schemes: BTreeMap<HirDefinitionId, TypeScheme>,
    pub diagnostics: Vec<Diagnostic>,
    pub types: TypeGraph,
}

impl Analysis {
    pub fn display(&self, id: AnalysisTypeId) -> String {
        self.types.display(id)
    }
}

pub fn analyze_source(source_name: &str, source: &str) -> Result<Analysis, FrontendError> {
    analyze_source_with_fuel(source_name, source, DEFAULT_TOOL_FUEL)
}

pub fn analyze_source_with_fuel(
    source_name: &str,
    source: &str,
    evaluation_fuel: usize,
) -> Result<Analysis, FrontendError> {
    analyze_source_with_quota(source_name, source, Quota::with_fuel(evaluation_fuel))
}

pub fn analyze_source_with_quota(
    source_name: &str,
    source: &str,
    quota: Quota,
) -> Result<Analysis, FrontendError> {
    let mut sources = SourceDatabase::default();
    let source_id = sources.add(source_name, source);
    let parsed = parse_registered(&sources, source_id);
    let program = parsed.program.ok_or_else(|| {
        FrontendError::from_diagnostic(
            &sources,
            parsed
                .diagnostics
                .into_iter()
                .next()
                .expect("failed parse has a diagnostic"),
        )
    })?;
    let mut account = QuotaAccount::new(quota);
    analyze_program_with_bindings(
        source_name,
        &program,
        &mut account,
        &BTreeMap::new(),
        &HashSet::new(),
        &sources,
        &BTreeMap::new(),
    )
}

pub fn analyze_partial_types(source_name: &str, source: &str, quota: Quota) -> PartialAnalysis {
    analyze_partial_types_with_bindings(source_name, source, quota, &BTreeMap::new())
}

pub fn analyze_partial_types_with_bindings(
    source_name: &str,
    source: &str,
    quota: Quota,
    external_values: &BTreeMap<String, crate::DataWorld>,
) -> PartialAnalysis {
    let mut sources = SourceDatabase::default();
    let source_id = sources.add(source_name, source);
    analyze_partial_types_registered(&sources, source_id, quota, external_values, &HashSet::new())
}

pub(crate) fn analyze_partial_types_registered(
    sources: &SourceDatabase,
    source_id: crate::SourceId,
    quota: Quota,
    external_values: &BTreeMap<String, crate::DataWorld>,
    unavailable_imports: &HashSet<String>,
) -> PartialAnalysis {
    let parsed = parse_registered(sources, source_id);
    analyze_partial_types_recovered(
        sources,
        source_id,
        &parsed.recovered,
        parsed.diagnostics,
        quota,
        external_values,
        unavailable_imports,
    )
}

pub(crate) fn analyze_partial_types_recovered(
    sources: &SourceDatabase,
    source_id: crate::SourceId,
    recovered: &crate::parser::RecoveredProgram,
    initial_diagnostics: Vec<Diagnostic>,
    quota: Quota,
    external_values: &BTreeMap<String, crate::DataWorld>,
    unavailable_imports: &HashSet<String>,
) -> PartialAnalysis {
    let mut tool_heap = Heap::main();
    let external_roots = external_values
        .iter()
        .filter_map(|(name, value)| {
            value
                .publish(&mut tool_heap)
                .ok()
                .map(|root| (name.clone(), root))
        })
        .collect();
    analyze_partial_types_recovered_with_query(
        sources,
        source_id,
        recovered,
        initial_diagnostics,
        quota,
        &external_roots,
        &mut tool_heap,
        PartialAnalysisControl {
            unavailable_imports,
            external_schemes: &BTreeMap::new(),
            query: None,
        },
    )
}

pub(crate) struct PartialAnalysisControl<'a> {
    pub unavailable_imports: &'a HashSet<String>,
    pub external_schemes: &'a BTreeMap<String, TypeScheme>,
    pub query: Option<&'a crate::query::QueryContext>,
}

pub(crate) fn analyze_partial_types_recovered_with_query(
    sources: &SourceDatabase,
    source_id: crate::SourceId,
    recovered: &crate::parser::RecoveredProgram,
    initial_diagnostics: Vec<Diagnostic>,
    quota: Quota,
    external_roots: &BTreeMap<String, PersistentValue>,
    tool_heap: &mut Heap,
    control: PartialAnalysisControl<'_>,
) -> PartialAnalysis {
    let source_name = sources.get(source_id).name.to_string();
    let module_id = crate::ModuleId::ANONYMOUS;
    let prelude = BootstrapPrelude::new();
    let hir = HirProgram::resolve_recovered(
        recovered,
        prelude
            .schemes
            .keys()
            .filter(|name| name.as_str() != "BlameError")
            .filter(|name| !external_roots.contains_key(*name))
            .chain(external_roots.keys())
            .cloned()
            .collect::<Vec<_>>(),
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
    let mut diagnostics = initial_diagnostics;
    let mut facts: BTreeMap<HirDefinitionId, SemanticFact<AnalysisTypeId>> = BTreeMap::new();
    let mut definition_schemes = BTreeMap::new();
    for (definition, import) in unavailable_dependencies {
        let cause = FactIdentity::HirDefinition(import);
        let mut fact = SemanticFact::unknown(UnknownReason::BlockedBy(cause));
        fact.causes.push(cause);
        facts.insert(definition, fact);
    }
    let mut types = TypeGraph::default();
    let debug_sink: Arc<dyn DebugSink> = Arc::new(DiscardDebugSink);
    let mut evaluator = ToolEvaluator::new(Arc::clone(&debug_sink), tool_heap);
    let mut tool_values = evaluator
        .install_bootstrap()
        .expect("core prelude values can enter the tool Main world");
    tool_values.extend(
        external_roots
            .iter()
            .map(|(name, root)| (name.clone(), root.runtime())),
    );
    let any_metadata = *tool_values.get("Any").expect("core prelude defines Any");
    for binding in bindings.values() {
        tool_values.insert(binding.value.name.value.clone(), any_metadata);
    }
    for node in &dependencies.nodes {
        let binding = bindings[&node.definition];
        if binding.value.type_parameters.is_empty()
            && binding.value.declared_initializer.is_some()
            && dependency_reaches(&dependencies, node.definition, node.definition)
        {
            let name = binding.value.name.value.clone();
            tool_values.insert(
                name.clone(),
                evaluator
                    .descriptor(&TypeDescriptor::Named(name.clone()))
                    .expect("named metadata can enter the tool Main world"),
            );
        }
    }
    let mut account = QuotaAccount::new(quota);
    if let Some(query) = control.query {
        account = account.with_query(query.clone());
    }
    while facts.len() < bindings.len() {
        let mut progressed = false;
        for node in &dependencies.nodes {
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
            let mut evaluation_bindings = tool_values.clone();
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
                evaluation_bindings.insert(
                    parameter.value.clone(),
                    evaluator
                        .descriptor(&TypeDescriptor::Bound(id))
                        .expect("bound metadata can enter the tool Main world"),
                );
            }
            if facts.contains_key(&node.definition) {
                progressed = true;
                continue;
            }
            let outcome = evaluate_tool_expression(
                &source_name,
                &binding.value.value,
                &evaluation_bindings,
                &mut account,
                sources,
                &mut evaluator,
            )
            .and_then(|value| {
                let value = if parameters.is_empty() {
                    declare_metadata_value(
                        &source_name,
                        module_id,
                        binding,
                        &declared_initializer_slots,
                        value,
                        &mut evaluator,
                    )?
                } else {
                    value
                };
                evaluator
                    .decode_type(value, "Type")
                    .map(|descriptor| {
                        let descriptor = if !parameters.is_empty()
                            && binding.value.declared_initializer.is_some()
                        {
                            let arguments = parameters
                                .iter()
                                .map(|parameter| TypeDescriptor::Bound(parameter.id))
                                .collect::<Vec<_>>();
                            TypeDescriptor::Declared(DeclaredTypeDescriptor {
                                id: crate::value::DeclaredTypeId::applied(
                                    module_id,
                                    declared_initializer_slots[&binding.value.name.location],
                                    &arguments,
                                ),
                                name: binding.value.name.value.clone(),
                                body: Arc::new(descriptor),
                            })
                        } else {
                            descriptor
                        };
                        (value, descriptor)
                    })
                    .map_err(|message| {
                        FrontendError::from_diagnostic(
                            sources,
                            Diagnostic::error(
                                format!(
                                    "type {} produced invalid metadata: {message}",
                                    binding.value.name.value
                                ),
                                binding.value.value.location,
                            ),
                        )
                    })
            });
            match outcome {
                Ok((value, descriptor)) => {
                    let (definition_descriptor, published_value) = if parameters.is_empty() {
                        let declared = types.intern_descriptor(&descriptor);
                        types
                            .names
                            .insert(binding.value.name.value.clone(), declared);
                        (descriptor, value)
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
                        let constructor = binding.value.declared_initializer.as_ref().map(|_| {
                            NominalTypeConstructor {
                                id: crate::TypeConstructorId {
                                    module: module_id,
                                    local: declared_initializer_slots[&binding.value.name.location],
                                },
                                name: binding.value.name.value.clone(),
                            }
                        });
                        let (family_value, template_root, family_root) = evaluator
                            .create_type_family(value, parameters.len(), constructor.as_ref())
                            .expect("type-family closure can enter the tool Main world");
                        let family = TypeFamilyTemplate {
                            parameters: parameters.clone(),
                            template: template_root,
                            root: family_root,
                            rebuild_at_runtime: contains_named_type(&descriptor),
                            constructor,
                        };
                        let scheme = TypeScheme {
                            parameters,
                            constraints: Vec::new(),
                            body: TypeDescriptor::Function {
                                parameters: family
                                    .parameters
                                    .iter()
                                    .map(|parameter| {
                                        TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Bound(
                                            parameter.id,
                                        )))
                                    })
                                    .collect(),
                                result: Box::new(TypeDescriptor::TypeOf(Box::new(descriptor))),
                            },
                        };
                        let erased = erase_type_variables(&scheme.body);
                        definition_schemes.insert(node.definition, scheme);
                        (erased, family_value)
                    };
                    let id = types.intern_descriptor(&definition_descriptor);
                    tool_values.insert(binding.value.name.value.clone(), published_value);
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
            .filter(|node| dependency_reaches(&dependencies, node.definition, node.definition))
            .map(|node| node.definition)
            .collect::<Vec<_>>();
        let had_cycle = !cyclic.is_empty();
        let mut handled = HashSet::new();
        for root in cyclic {
            if !handled.insert(root) {
                continue;
            }
            let component = dependencies
                .nodes
                .iter()
                .map(|node| node.definition)
                .filter(|definition| !facts.contains_key(definition))
                .filter(|definition| {
                    dependency_reaches(&dependencies, root, *definition)
                        && dependency_reaches(&dependencies, *definition, root)
                })
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
                let outcome = build_recursive_type_family(
                    &source_name,
                    module_id,
                    declared_initializer_slots[&binding.value.name.location],
                    binding,
                    &tool_values,
                    &mut account,
                    sources,
                    &mut evaluator,
                );
                match outcome {
                    Ok(built) => {
                        let descriptor = erase_type_variables(&built.scheme.body);
                        let id = types.intern_descriptor(&descriptor);
                        definition_schemes.insert(definition, built.scheme);
                        tool_values.insert(binding.value.name.value.clone(), built.family_value);
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
                let mut descriptors = BTreeMap::new();
                let mut values = BTreeMap::new();
                let mut failed = false;
                for definition in &component {
                    let binding = bindings[definition];
                    let outcome = evaluate_tool_expression(
                        &source_name,
                        &binding.value.value,
                        &tool_values,
                        &mut account,
                        sources,
                        &mut evaluator,
                    )
                    .and_then(|value| {
                        evaluator
                            .decode_type(value, "Type")
                            .map(|descriptor| (value, descriptor))
                            .map_err(|message| {
                                FrontendError::from_diagnostic(
                                    sources,
                                    Diagnostic::error(
                                        format!(
                                            "type {} produced invalid metadata: {message}",
                                            binding.value.name.value
                                        ),
                                        binding.value.value.location,
                                    ),
                                )
                            })
                    });
                    match outcome {
                        Ok((value, descriptor)) => {
                            descriptors.insert(binding.value.name.value.clone(), descriptor);
                            values.insert(binding.value.name.value.clone(), value);
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
                            facts.insert(*definition, fact);
                            failed = true;
                        }
                    }
                }
                if !failed {
                    let roots = types.install_named_descriptors(&descriptors);
                    for definition in &component {
                        let binding = bindings[definition];
                        let name = &binding.value.name.value;
                        facts.insert(*definition, SemanticFact::known(roots[name]));
                        tool_values.insert(name.clone(), values[name]);
                    }
                } else {
                    for definition in &component {
                        facts.entry(*definition).or_insert_with(|| {
                            SemanticFact::unknown(UnknownReason::BlockedBy(
                                FactIdentity::HirDefinition(root),
                            ))
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
        let Some(root) = external_roots.get(&definition.name) else {
            continue;
        };
        if root.runtime().type_id().is_none() {
            continue;
        }
        let descriptor = infer_value_ref(ValueRef::persistent(*root, evaluator.main));
        let ty = types.intern_descriptor(&descriptor);
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
