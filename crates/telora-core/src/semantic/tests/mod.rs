use super::*;

#[test]
fn semantic_interface_consumes_the_solved_graph_without_renumbering() {
    use crate::types::{TypeDescriptor, TypeNode, TypeScheme};
    let mut types = TypeGraph::default();
    types.intern_module_interface(&ModuleInterface {
        concrete_types: BTreeMap::from([
            ("Retained".into(), TypeDescriptor::Bytes),
            ("Element".into(), TypeDescriptor::Int),
            ("Items".into(), TypeDescriptor::Array(Box::new(TypeDescriptor::Int))),
        ]),
        ..Default::default()
    });
    let retained = types.named("Retained").unwrap();
    let int = types.named("Element").unwrap();
    let array = types.named("Items").unwrap();
    let count = types.nodes().len();
    let interface = ModuleInterface {
        exports: BTreeMap::from([("items".into(), TypeScheme {
            parameters: Vec::new(), constraints: Vec::new(),
            body: TypeDescriptor::Array(Box::new(TypeDescriptor::Int)),
        })]),
        ..Default::default()
    };
    let snapshot = SemanticModuleInterface::with_types(&interface, types);
    assert_eq!(snapshot.types.node(retained), &TypeNode::Bytes);
    assert_eq!(snapshot.types.node(array), &TypeNode::Array(int));
    let TypeNode::Struct(fields) = snapshot.types.node(snapshot.result_type) else { panic!("exports") };
    assert_eq!(fields["items"], array);
    assert_eq!(snapshot.types.nodes().len(), count + 1);
}
