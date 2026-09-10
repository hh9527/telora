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


impl Analysis {
    pub fn display(&self, id: AnalysisTypeId) -> String {
        self.types.display(id)
    }
}
