#[derive(Clone, Debug)]
pub struct Analysis {
    pub types: TypeGraph,
    pub declared_types: BTreeMap<String, AnalysisTypeId>,
    pub binding_types: BTreeMap<String, AnalysisTypeId>,
    pub trait_ids: BTreeMap<String, crate::TraitId>,
    pub trait_implementations: Vec<TraitImplementation>,
    pub result_type: AnalysisTypeId,
    pub(crate) result_scheme: Option<TypeScheme>,
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
    pub(crate) declared_value_owners: HashMap<crate::Location, ResolvedEvidence>,
    pub(crate) value_constructors: HashMap<crate::Location, ValueConstructor>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ValueConstructor {
    Newtype,
    EnumMember { tag: String, has_payload: bool },
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

impl PartialAnalysis {
    // Preserve source identities when no typed artifact was produced. This is
    // a diagnostic envelope, never a request to rerun resolution or inference.
    pub(crate) fn from_resolved(hir: HirProgram) -> Self {
        Self { hir, dependencies: Default::default(), definition_facts: BTreeMap::new(),
            definition_schemes: BTreeMap::new(), diagnostics: Vec::new(), types: Default::default() }
    }
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
    _quota: Quota,
    external_values: &BTreeMap<String, crate::DataWorld>,
    unavailable_imports: &HashSet<String>,
) -> PartialAnalysis {
    let interfaces = external_values.iter().filter_map(|(name, value)|
        value.static_interface(name).map(|interface| (name.clone(), interface)))
        .collect::<BTreeMap<_, _>>();
    let imported_types = interfaces.iter().filter_map(|(name, interface)|
        imported_interface_descriptor(interface).map(|ty| (name.clone(), ty)))
        .collect::<HashMap<_, _>>();
    solve_partial_types(
        sources, source_id, recovered, initial_diagnostics,
        PartialTypeInputs {
            names: external_values.keys().cloned().collect(),
            imported_values: imported_types.iter().map(|(name, ty)| (name.clone(), ty.clone())).collect(),
            imported_types, interfaces: interfaces.clone(), ..Default::default()
        },
        PartialAnalysisControl {
            unavailable_imports,
            external_schemes: &BTreeMap::new(),
            external_interfaces: &interfaces,
            query: None,
        },
    )
}

pub(crate) struct PartialAnalysisControl<'a> {
    pub unavailable_imports: &'a HashSet<String>,
    pub external_schemes: &'a BTreeMap<String, TypeScheme>,
    pub external_interfaces: &'a BTreeMap<String, ModuleInterface>,
    pub query: Option<&'a crate::query::QueryContext>,
}
