pub(crate) fn relocate_work_roots(
    target: &mut Heap,
    main: &Heap,
    source: &Heap,
    roots: &[Val],
) -> Result<Vec<Val>, HeapError> {
    if target.storage != Storage::Work
        || source.storage != Storage::Work
        || main.storage != Storage::Main
    {
        return Err(HeapError(
            "work relocation requires two Work worlds and one Main world",
        ));
    }
    // FuncRef is an immediate up-link, so move the source's sealed targets
    // beside the copied graph when the Main world does not already own them.
    let mut functions = source
        .functions
        .iter()
        .filter_map(|(id, value)| value.map(|value| (*id, value)))
        .filter(|(id, _)| target.static_func(*id).is_none() && main.static_func(*id).is_none())
        .collect::<Vec<_>>();
    functions.sort_by_key(|(id, _)| *id);
    let mut copy = Vec::with_capacity(roots.len() + functions.len());
    copy.extend_from_slice(roots);
    copy.extend(functions.iter().map(|(_, value)| *value));
    let copied = copy_roots(
        target,
        HeapView {
            current: source,
            background: Some(main),
        },
        &copy,
    )?;
    for ((id, _), value) in functions
        .into_iter()
        .zip(copied.iter().skip(roots.len()).copied())
    {
        target.seal_static_func(id, value)?;
    }
    Ok(copied[..roots.len()].to_vec())
}

pub(crate) fn publish_root(
    target: &mut Heap,
    current: &Heap,
    root: Val,
) -> Result<PersistentValue, HeapError> {
    if target.storage != Storage::Main || current.storage != Storage::Work {
        return Err(HeapError(
            "publication requires a Work world and Main world",
        ));
    }
    if (HeapView {
        current,
        background: Some(target),
    })
    .first_data_failure(root)?
    .is_some()
    {
        return Err(HeapError(
            "failed evaluation node cannot cross a Host publication boundary",
        ));
    }
    let roots = copy_roots(
        target,
        HeapView {
            current,
            background: None,
        },
        &[root],
    )?;
    Ok(PersistentValue(roots[0]))
}

pub(crate) fn publish_type_properties(
    target: &mut Heap,
    current: &Heap,
    property_attr_type: Option<crate::TypeId>,
    properties: &[(PropertyKey, Val)],
) -> Result<(), HeapError> {
    if target.storage != Storage::Main || current.storage != Storage::Work {
        return Err(HeapError(
            "type property publication requires a Work world and Main world",
        ));
    }
    let mut keys = HashSet::new();
    let view = HeapView {
        current,
        background: Some(target),
    };
    for (key, value) in properties {
        if !keys.insert(*key) {
            return Err(HeapError("duplicate typed property for target"));
        }
        if value.type_id() != Some(key.property_type()) {
            return Err(HeapError(
                "typed property runtime witness does not match its property TypeId",
            ));
        }
        if view.first_data_failure(*value)?.is_some() {
            return Err(HeapError(
                "failed evaluation node cannot be published as a typed property",
            ));
        }
        if let Some(existing) = target.properties.get(key)
            && !view.values_equal(*value, *existing)?
        {
            return Err(HeapError("conflicting typed property for target"));
        }
    }
    if let Some(marker) = property_attr_type {
        match target.property_attr_type {
            Some(existing) if existing != marker => {
                return Err(HeapError("PropertyAttr TypeId is already established"));
            }
            _ => {}
        }
    }
    let unpublished = properties
        .iter()
        .filter(|(key, _)| !target.properties.contains_key(key))
        .collect::<Vec<_>>();
    let roots = unpublished
        .iter()
        .map(|(_, value)| *value)
        .collect::<Vec<_>>();
    let copied = copy_roots(
        target,
        HeapView {
            current,
            background: None,
        },
        &roots,
    )?;
    for ((key, _), value) in unpublished.into_iter().zip(copied.into_iter()) {
        target.properties.insert(*key, value);
    }
    if let Some(marker) = property_attr_type {
        target.property_attr_type = Some(marker);
    }
    Ok(())
}

pub(crate) fn publish_module_root(
    target: &mut Heap,
    current: &Heap,
    root: Val,
) -> Result<PersistentValue, HeapError> {
    if target.storage != Storage::Main || current.storage != Storage::Work {
        return Err(HeapError(
            "module publication requires a Work world and Main world",
        ));
    }
    let mut functions = current
        .functions
        .iter()
        .filter_map(|(id, value)| value.map(|value| (*id, value)))
        .collect::<Vec<_>>();
    functions.sort_by_key(|(id, _)| *id);
    for (id, _) in &functions {
        match target.functions.get(id) {
            Some(None) => {}
            Some(Some(_)) => return Err(HeapError("static function slot is already sealed")),
            None => return Err(HeapError("unknown static function slot")),
        }
    }
    let mut roots = Vec::with_capacity(functions.len() + 1);
    roots.push(root);
    roots.extend(functions.iter().map(|(_, value)| *value));
    let copied = copy_roots(
        target,
        HeapView {
            current,
            background: None,
        },
        &roots,
    )?;
    for ((id, _), value) in functions.into_iter().zip(copied.iter().skip(1).copied()) {
        let slot = target
            .functions
            .get_mut(&id)
            .expect("static function slots were validated before copying");
        debug_assert!(slot.is_none());
        *slot = Some(value);
    }
    Ok(PersistentValue(copied[0]))
}
