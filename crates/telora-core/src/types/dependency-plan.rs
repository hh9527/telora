fn expression_dependencies(hir: &HirProgram, root: HirExpressionId) -> Vec<HirDefinitionId> {
    expression_dependencies_with_properties(hir, root, true)
}

fn expression_dependencies_with_properties(
    hir: &HirProgram,
    root: HirExpressionId,
    include_properties: bool,
) -> Vec<HirDefinitionId> {
    let mut dependencies = Vec::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        let expression = hir.expression(id).expect("HIR expression exists");
        if !include_properties && hir.is_property_root(expression.location) { continue; }
        if let Some(reference) = expression.reference.and_then(|id| hir.reference(id))
            && let HirResolution::Definition(dependency) = reference.resolution
        {
            dependencies.push(dependency);
        }
        pending.extend(hir.expression_children(id));
    }
    dependencies.sort_unstable();
    dependencies.dedup();
    dependencies
}

fn definition_dependencies(hir: &HirProgram, definition: HirDefinitionId) -> Vec<HirDefinitionId> {
    let root = hir
        .definition(definition)
        .and_then(|definition| definition.value)
        .expect("type definition has a value expression");
    expression_dependencies(hir, root)
}

fn type_definition_dependencies(hir: &HirProgram, definition: HirDefinitionId) -> Vec<HirDefinitionId> {
    let root = hir.definition(definition).and_then(|definition| definition.value)
        .expect("type definition has a value expression");
    expression_dependencies_with_properties(hir, root, false)
}

fn type_dependency_graph(
    hir: &HirProgram,
    type_definitions: &HashSet<HirDefinitionId>,
) -> SemanticDependencyGraph {
    let mut nodes = type_definitions
        .iter()
        .copied()
        .map(|definition| SemanticDependencyNode {
            definition,
            dependencies: type_definition_dependencies(hir, definition)
                .into_iter()
                .filter(|dependency| type_definitions.contains(dependency))
                .collect(),
        })
        .collect::<Vec<_>>();
    nodes.sort_by_key(|node| node.definition);
    SemanticDependencyGraph { nodes }
}

struct TypeDependencyPlan<'a> {
    graph: &'a SemanticDependencyGraph,
    positions: HashMap<HirDefinitionId, usize>,
    reverse: Vec<Vec<usize>>,
    components: Vec<Vec<HirDefinitionId>>,
    component_ids: Vec<usize>,
    cyclic: Vec<bool>,
}

#[cfg(test)]
mod dependency_plan_tests {
    use super::*;

    fn definitions(count: usize) -> Vec<HirDefinitionId> {
        let source = (0..count).map(|index| format!("type T{index} = Int;"))
            .collect::<Vec<_>>().join("\n");
        let program = crate::parser::parse("dependencies.telora", &source).unwrap();
        HirProgram::resolve(&program, ["Int".into()]).definitions().iter()
            .filter(|definition| definition.top_level).map(|definition| definition.id).collect()
    }

    #[test]
    fn components_and_schedule_match_reachability_for_all_three_node_graphs() {
        let ids = definitions(3);
        for mask in 0..512 {
            let mut reaches = [[false; 3]; 3];
            let nodes = ids.iter().enumerate().map(|(source, &definition)| {
                let dependencies = ids.iter().enumerate().filter_map(|(target, &id)| {
                    let edge = mask & (1 << (source * 3 + target)) != 0;
                    reaches[source][target] = edge;
                    edge.then_some(id)
                }).collect();
                SemanticDependencyNode { definition, dependencies }
            }).collect();
            for via in 0..3 {
                for source in 0..3 {
                    for target in 0..3 {
                        reaches[source][target] |= reaches[source][via] && reaches[via][target];
                    }
                }
            }
            let graph = SemanticDependencyGraph { nodes };
            let plan = TypeDependencyPlan::new(&graph);
            let order = plan.order(&ids.iter().copied().collect());
            let ranks = order.iter().enumerate().flat_map(|(rank, component)| {
                component.iter().map(move |&id| (id, rank))
            }).collect::<HashMap<_, _>>();
            assert_eq!(ranks.len(), 3);
            for source in 0..3 {
                assert_eq!(plan.is_cyclic(ids[source]), reaches[source][source]);
                let dependents = plan.dependents(&[ids[source]]);
                for target in 0..3 {
                    assert_eq!(plan.component(ids[source]).contains(&ids[target]),
                        source == target || reaches[source][target] && reaches[target][source]);
                    assert_eq!(dependents.contains(&ids[target]), source == target || reaches[target][source]);
                    if reaches[source][target] { assert!(ranks[&ids[target]] <= ranks[&ids[source]]); }
                }
            }
            let selected = ids.iter().enumerate().filter_map(|(index, &id)| {
                (index == 0 || reaches[0][index]).then_some(id)
            }).collect::<BTreeSet<_>>();
            assert_eq!(plan.order(&selected).into_iter().flatten().collect::<BTreeSet<_>>(), selected);
        }
    }

    #[test]
    fn long_forward_chain_is_scheduled_dependency_first() {
        let ids = definitions(2048);
        let graph = SemanticDependencyGraph { nodes: ids.iter().enumerate().map(|(index, &definition)| {
            SemanticDependencyNode { definition, dependencies: ids.get(index + 1).copied().into_iter().collect() }
        }).collect() };
        let plan = TypeDependencyPlan::new(&graph);
        let order = plan.order(&ids.iter().copied().collect()).into_iter().flatten().collect::<Vec<_>>();
        assert_eq!(order, ids.iter().rev().copied().collect::<Vec<_>>());
    }

    #[test]
    fn indexed_subtrees_match_parent_walks_including_decorators() {
        let program = crate::parser::parse("dependencies.telora", r#"
            def mark: Fn(Type, Option(Int)) -> Int = fn(target, previous) { 1 };
            @mark
            type A = struct {value: B};
            type B = Int;
            let f = fn(B: Type) { let inner = fn(x) { B }; inner(B) };
        "#).unwrap();
        let hir = HirProgram::resolve(&program, ["Int".into(), "Type".into(), "Fn".into(), "Option".into()]);
        for root in hir.expressions() {
            let at = hir.expressions().iter().filter(|expression| expression.location == root.location)
                .map(|expression| expression.id).collect::<Vec<_>>();
            assert_eq!(hir.expression_ids_at(root.location).collect::<Vec<_>>(), at);
            for include_properties in [false, true] {
                let mut expected = Vec::new();
                for expression in hir.expressions() {
                    let mut parent = Some(expression.id);
                    while let Some(id) = parent {
                        let current = hir.expression(id).unwrap();
                        if !include_properties && hir.is_property_root(current.location) { break; }
                        if id == root.id {
                            if let Some(reference) = expression.reference.and_then(|id| hir.reference(id))
                                && let HirResolution::Definition(dependency) = reference.resolution
                            { expected.push(dependency); }
                            break;
                        }
                        parent = current.parent;
                    }
                }
                expected.sort_unstable();
                expected.dedup();
                assert_eq!(expression_dependencies_with_properties(&hir, root.id, include_properties), expected);
            }
        }
    }
}

impl<'a> TypeDependencyPlan<'a> {
    fn new(graph: &'a SemanticDependencyGraph) -> Self {
        let positions = graph.nodes.iter().enumerate()
            .map(|(index, node)| (node.definition, index)).collect::<HashMap<_, _>>();
        let edges = graph.nodes.iter().map(|node| node.dependencies.iter()
            .map(|dependency| positions[dependency]).collect::<Vec<_>>()).collect::<Vec<_>>();
        let mut reverse = vec![Vec::new(); edges.len()];
        for (source, targets) in edges.iter().enumerate() {
            for &target in targets { reverse[target].push(source); }
        }

        // Iterative Kosaraju traversal keeps long forward-reference chains off
        // the Rust call stack. Components are subsequently scheduled as units.
        let mut visited = vec![false; edges.len()];
        let mut finished = Vec::new();
        for root in 0..edges.len() {
            let mut pending = vec![(root, false)];
            while let Some((node, exiting)) = pending.pop() {
                if exiting { finished.push(node); continue; }
                if std::mem::replace(&mut visited[node], true) { continue; }
                pending.push((node, true));
                pending.extend(edges[node].iter().rev().map(|&next| (next, false)));
            }
        }
        let mut component_ids = vec![usize::MAX; edges.len()];
        let mut components = Vec::new();
        for root in finished.into_iter().rev() {
            if component_ids[root] != usize::MAX { continue; }
            let mut component = Vec::new();
            let mut pending = vec![root];
            while let Some(node) = pending.pop() {
                if component_ids[node] != usize::MAX { continue; }
                component_ids[node] = components.len();
                component.push(graph.nodes[node].definition);
                pending.extend(&reverse[node]);
            }
            component.sort_unstable();
            components.push(component);
        }
        let cyclic = components.iter().map(|component| {
            component.len() > 1 || graph.nodes[positions[&component[0]]]
                .dependencies.contains(&component[0])
        }).collect();
        Self { graph, positions, reverse, components, component_ids, cyclic }
    }

    fn node(&self, definition: HirDefinitionId) -> &SemanticDependencyNode {
        &self.graph.nodes[self.positions[&definition]]
    }

    fn is_cyclic(&self, definition: HirDefinitionId) -> bool {
        self.cyclic[self.component_ids[self.positions[&definition]]]
    }

    fn component(&self, definition: HirDefinitionId) -> &[HirDefinitionId] {
        &self.components[self.component_ids[self.positions[&definition]]]
    }

    fn dependents(&self, roots: &[HirDefinitionId]) -> HashSet<HirDefinitionId> {
        let mut visited = HashSet::new();
        let mut pending = roots.iter().map(|root| self.positions[root]).collect::<Vec<_>>();
        while let Some(node) = pending.pop() {
            if visited.insert(self.graph.nodes[node].definition) {
                pending.extend(&self.reverse[node]);
            }
        }
        visited
    }

    fn order(&self, selected: &BTreeSet<HirDefinitionId>) -> Vec<Vec<HirDefinitionId>> {
        let mut active = vec![false; self.components.len()];
        for definition in selected {
            active[self.component_ids[self.positions[definition]]] = true;
        }
        let mut remaining = vec![0; self.components.len()];
        let mut dependents = vec![Vec::new(); self.components.len()];
        let mut ready = BTreeSet::new();
        for (index, component) in self.components.iter().enumerate() {
            if !active[index] { continue; }
            let mut dependencies = HashSet::new();
            for definition in component {
                assert!(selected.contains(definition), "scheduled SCC must be complete");
                for dependency in &self.node(*definition).dependencies {
                    let target = self.component_ids[self.positions[dependency]];
                    assert!(active[target], "scheduled types must include their dependencies");
                    if target != index { dependencies.insert(target); }
                }
            }
            remaining[index] = dependencies.len();
            for dependency in dependencies { dependents[dependency].push(index); }
            if remaining[index] == 0 { ready.insert((component[0], index)); }
        }
        let mut ordered = Vec::new();
        while let Some((_, index)) = ready.pop_first() {
            ordered.push(self.components[index].clone());
            for &dependent in &dependents[index] {
                remaining[dependent] -= 1;
                if remaining[dependent] == 0 {
                    ready.insert((self.components[dependent][0], dependent));
                }
            }
        }
        assert_eq!(ordered.len(), active.iter().filter(|active| **active).count());
        ordered
    }
}
