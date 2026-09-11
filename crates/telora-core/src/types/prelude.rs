pub(crate) fn unchecked_type_constructor() -> crate::TypeConstructorId {
    crate::TypeConstructorId { module: crate::ModuleId::ANONYMOUS, local: 2 }
}

pub(crate) fn canonical_type_ref_id(
    value: ValueRef<'_>,
    path: &str,
    types: &crate::type_store::SharedTypeStore,
) -> Result<TypeId, String> {
    let mut graph = TypeGraph::default();
    let root = graph.decode_persistent(value, path, &mut HashMap::new())?;
    let mut types = types
        .lock()
        .map_err(|_| "type store poisoned".to_owned())?;
    graph.canonicalize(root, &mut types)
}
