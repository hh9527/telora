struct GenericInference<'a> {
    schemes: HashMap<String, TypeScheme>,
    scheme_scopes: Vec<HashMap<String, Option<TypeScheme>>>,
    top_level_inferred_schemes: HashMap<String, TypeScheme>,
    inferred_schemes: HashMap<crate::Location, TypeScheme>,
    placeholder_obligations: Vec<(InferenceVariableId, crate::Location, String)>,
    hir: &'a HirProgram,
    external_interfaces: &'a BTreeMap<String, ModuleInterface>,
    named_types: &'a BTreeMap<String, TypeDescriptor>,
    declared_bodies: HashMap<crate::value::DeclaredTypeId, Arc<TypeDescriptor>>,
    local_annotations: &'a HashMap<crate::Location, TypeDescriptor>,
    authored_any_definitions: HashSet<crate::Location>,
    dyn_namespaces: &'a HashSet<String>,
    builtin_tuple_available: bool,
    query: Option<crate::query::QueryContext>,
    next_variable: u32,
    closure_inference_depth: usize,
    delayed_initializer_depth: usize,
    recursive_body_inference_depth: usize,
    numeric_variables: HashSet<InferenceVariableId>,
    not_variables: HashSet<InferenceVariableId>,
    ordered_variables: HashSet<InferenceVariableId>,
    field_requirements: HashMap<InferenceVariableId, BTreeMap<String, TypeDescriptor>>,
    recursive_equations: HashMap<InferenceVariableId, TypeDescriptor>,
    substitutions: HashMap<InferenceVariableId, TypeDescriptor>,
    records: HashMap<crate::Location, TypeDescriptor>,
    pattern_diagnostics: BTreeMap<crate::Location, String>,
    pattern_binding_types: HashMap<crate::Location, TypeDescriptor>,
    propagation_boundaries: Vec<Option<PropagationRequirement>>,
    return_boundaries: Vec<Option<ReturnBoundary>>,
    propagation_families: HashMap<crate::Location, PropagationFamily>,
    not_families: HashMap<crate::Location, NotFamily>,
    failure_location: Option<crate::Location>,
    failure_expected_location: Option<crate::Location>,
    enum_failure: Option<EnumInferenceFailure>,
    checking_named_pairs: HashSet<(String, String)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EnumInferenceFailureKind {
    IllegalVariant,
    MissingContext,
    Payload,
}

#[derive(Clone, Debug)]
struct EnumInferenceFailure {
    kind: EnumInferenceFailureKind,
    expected_name: String,
}

#[derive(Clone)]
enum PropagationRequirement {
    Option,
    Result(Vec<TypeDescriptor>),
}

struct ReturnBoundary {
    expected: Option<TypeDescriptor>,
    values: Vec<TypeDescriptor>,
}

#[derive(Default)]
struct DefinitionComponentPlan {
    recursive: HashSet<crate::Location>,
    indirect_recursive: HashSet<crate::Location>,
    acyclic: Vec<crate::Location>,
}

fn definition_component_plan(block: &Block, hir: &HirProgram) -> DefinitionComponentPlan {
    let candidates = block
        .value
        .bindings
        .iter()
        .filter(|binding| {
            binding.value.kind == BindingKind::Def
                && binding.value.annotation.is_none()
                && binding.value.type_parameters.is_empty()
                && matches!(binding.value.value.value, ExprKind::Closure { .. })
                && !block.value.bindings.iter().any(|candidate| {
                    candidate.value.kind == BindingKind::Decl
                        && candidate.value.name.value == binding.value.name.value
                })
        })
        .filter_map(|binding| {
            hir.definitions()
                .iter()
                .find(|definition| {
                    definition.kind == HirDefinitionKind::DefinitionSlot
                        && definition.name == binding.value.name.value
                        && definition.value.is_some_and(|value| {
                            hir.expression(value).is_some_and(|expression| {
                                expression.location == binding.value.value.location
                            })
                        })
                })
                .map(|definition| (definition.id, binding.value.name.location))
        })
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return DefinitionComponentPlan::default();
    }
    let indices = candidates
        .iter()
        .enumerate()
        .map(|(index, (definition, _))| (*definition, index))
        .collect::<HashMap<_, _>>();
    let direct_dependencies = hir
        .definitions()
        .iter()
        .filter_map(|definition| {
            let root = definition.value?;
            let mut dependencies = Vec::new();
            for expression in hir.expressions() {
                let Some(reference) = expression.reference.and_then(|id| hir.reference(id)) else {
                    continue;
                };
                let HirResolution::Definition(target) = reference.resolution else {
                    continue;
                };
                let mut owner = Some(expression.id);
                while let Some(current) = owner {
                    if current == root {
                        if !dependencies.contains(&target) {
                            dependencies.push(target);
                        }
                        break;
                    }
                    owner = hir
                        .expression(current)
                        .and_then(|expression| expression.parent);
                }
            }
            dependencies.sort_unstable();
            Some((definition.id, dependencies))
        })
        .collect::<HashMap<_, _>>();
    let mut edges = vec![Vec::new(); candidates.len()];
    let mut indirect_edge_sources = HashSet::new();
    for (index, (definition, _)) in candidates.iter().enumerate() {
        let mut pending = direct_dependencies
            .get(definition)
            .into_iter()
            .flatten()
            .map(|dependency| (*dependency, false))
            .collect::<Vec<_>>();
        let mut visited = HashSet::new();
        while let Some((target, indirect)) = pending.pop() {
            if !visited.insert((target, indirect)) {
                continue;
            }
            if let Some(&target) = indices.get(&target) {
                if !edges[index].contains(&target) {
                    edges[index].push(target);
                }
                if indirect {
                    indirect_edge_sources.insert(index);
                }
                continue;
            }
            if let Some(dependencies) = direct_dependencies.get(&target) {
                pending.extend(dependencies.iter().map(|dependency| (*dependency, true)));
            }
        }
        edges[index].sort_unstable();
    }

    fn reaches(
        current: usize,
        target: usize,
        edges: &[Vec<usize>],
        visited: &mut HashSet<usize>,
    ) -> bool {
        visited.insert(current)
            && edges[current]
                .iter()
                .any(|next| *next == target || reaches(*next, target, edges, visited))
    }

    let recursive_indices = (0..candidates.len())
        .filter(|index| reaches(*index, *index, &edges, &mut HashSet::new()))
        .collect::<HashSet<_>>();
    let mut ordered = Vec::new();
    let mut visited = HashSet::new();
    fn visit(
        node: usize,
        edges: &[Vec<usize>],
        recursive: &HashSet<usize>,
        visited: &mut HashSet<usize>,
        ordered: &mut Vec<usize>,
    ) {
        if recursive.contains(&node) || !visited.insert(node) {
            return;
        }
        for dependency in &edges[node] {
            visit(*dependency, edges, recursive, visited, ordered);
        }
        ordered.push(node);
    }
    for node in 0..candidates.len() {
        visit(node, &edges, &recursive_indices, &mut visited, &mut ordered);
    }
    DefinitionComponentPlan {
        recursive: recursive_indices
            .iter()
            .map(|index| candidates[*index].1)
            .collect(),
        indirect_recursive: recursive_indices
            .intersection(&indirect_edge_sources)
            .map(|index| candidates[*index].1)
            .collect(),
        acyclic: ordered
            .into_iter()
            .map(|index| candidates[index].1)
            .collect(),
    }
}

