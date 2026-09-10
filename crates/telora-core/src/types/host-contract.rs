// Source data carries its type before any runtime heap is materialized. Build
// canonical graph rows so nested array comparison does not clone type trees.
pub(crate) fn data_plan_contract(plan: &crate::json::ValidatedDataPlan) -> Option<TypeDescriptor> {
    use crate::json::{DataPlanNodeKind as N, DataScalar as S};
    let mut graph = TypeGraph::default();
    let mut roots = HashMap::new();
    let mut work = vec![(plan.root(), false)];
    while let Some((id, finish)) = work.pop() {
        if roots.contains_key(&id) { continue; }
        let node = &plan.node(id).kind;
        if !finish {
            work.push((id, true));
            match node {
                N::Array(items) => work.extend(items.iter().rev().map(|id| (*id, false))),
                N::Object(fields) => work.extend(fields.values().rev().map(|field| (field.value, false))),
                N::Scalar(_) => {}
            }
            continue;
        }
        let shape = match node {
            N::Scalar(S::Int(_)) => Some(TypeNode::Int),
            N::Scalar(S::Float(_)) => Some(TypeNode::Float),
            N::Scalar(S::String(_)) => Some(TypeNode::String),
            N::Scalar(S::Bytes(_)) => Some(TypeNode::Bytes),
            // Untyped surface tags have no inferred owner, as at the existing
            // Host data boundary. Do not manufacture an enum contract for them.
            N::Scalar(S::Atom(_) | S::TaggedString { .. }) => None,
            N::Array(items) => {
                let children = items.iter().map(|id| roots[id]).collect::<Option<Vec<_>>>();
                children.and_then(|children| {
                    let item = children.first().copied().unwrap_or_else(|| graph.intern_node(TypeNode::Never));
                    children.iter().all(|child| *child == item).then_some(TypeNode::Array(item))
                })
            }
            N::Object(fields) => fields.iter().map(|(name, field)| roots[&field.value].map(|id| (name.clone(), id)))
                .collect::<Option<BTreeMap<_, _>>>().map(TypeNode::Struct),
        };
        roots.insert(id, shape.map(|shape| graph.intern_node(shape)));
    }
    graph.descriptor(roots[&plan.root()]?).ok()
}

#[cfg(test)]
mod host_contract_tests {
    use super::*;
    use crate::json::{DataField, DataPlanNodeKind, DataScalar, ValidatedDataPlan};

    #[test]
    fn source_plan_contract_handles_shared_and_forward_edges_without_heap_values() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("host-contract", "");
        let location = crate::Location::from_usize(source, 0..0).unwrap();
        let mut plan = ValidatedDataPlan::default();
        let record = plan.object(BTreeMap::new(), location);
        let value = plan.scalar(DataScalar::Int(1), location);
        let DataPlanNodeKind::Object(fields) = &mut plan.node_mut(record).kind else { unreachable!() };
        fields.insert("value".into(), DataField { key_location: location, value });
        let array = plan.array(vec![record, record], location);
        plan.set_root(array);
        assert_eq!(data_plan_contract(&plan), Some(TypeDescriptor::Array(Box::new(TypeDescriptor::Struct(
            BTreeMap::from([("value".into(), TypeDescriptor::Int)]),
        )))));
        let string = plan.scalar(DataScalar::String("x".into()), location);
        let mixed = plan.array(vec![value, string], location);
        plan.set_root(mixed);
        assert_eq!(data_plan_contract(&plan), None);
        let atom = plan.scalar(DataScalar::Atom("None".into()), location);
        plan.set_root(atom);
        assert_eq!(data_plan_contract(&plan), None);
        let empty = plan.array(Vec::new(), location);
        plan.set_root(empty);
        assert_eq!(data_plan_contract(&plan), Some(TypeDescriptor::Array(Box::new(TypeDescriptor::Never))));
    }

    #[test]
    fn public_host_data_contracts_preserve_source_shapes() {
        let ints = TypeDescriptor::Array(Box::new(TypeDescriptor::Int));
        let mut values = vec![(crate::DataWorld::int(1), Some(TypeDescriptor::Int)),
            (crate::DataWorld::float(1.5).unwrap(), Some(TypeDescriptor::Float)),
            (crate::DataWorld::string("x"), Some(TypeDescriptor::String))];
        for (source, expected) in [("1", Some(TypeDescriptor::Int)),
            ("[]", Some(TypeDescriptor::Array(Box::new(TypeDescriptor::Never)))),
            ("[1,2]", Some(ints.clone())), ("[1,\"x\"]", None),
            ("{\"a\":[1,2]}", Some(TypeDescriptor::Struct(BTreeMap::from([("a".into(), ints)])))),
            ("true", None), ("null", None)] {
            values.push((crate::parse_json("host.json", source).unwrap(), expected));
        }
        for (value, expected) in values {
            let contract = value.static_interface("input").and_then(|interface| imported_interface_descriptor(&interface));
            assert_eq!(contract, expected);
        }
    }
}
