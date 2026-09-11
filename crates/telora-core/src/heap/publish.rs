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
    copy_roots(
        target,
        HeapView {
            current: source,
            background: Some(main),
        },
        roots,
    )
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

/// Copy a completed initialization graph into its own MainWorld. One forwarding
/// table spans every export/property root, preserving sharing and recursive
/// closures. Commit only after the entire object graph has been validated.
pub(crate) fn publish_initialized_roots(
    main: &mut Heap,
    work: &Heap,
    roots: &[Val],
) -> Result<Vec<Val>, HeapError> {
    if main.storage != Storage::Main || work.storage != Storage::Work || main.solved_types.is_none() {
        return Err(HeapError("initialization publication requires a typed Main world and a Work world"));
    }
    let source = HeapView { current: work, background: Some(main) };
    let mut pending = PendingCopy::new(main, &source);
    let copied = roots.iter().map(|root| pending.copy_value(main, &source, *root))
        .collect::<Result<Vec<_>, _>>()?;
    pending.validate()?;
    pending.commit(main);
    Ok(copied)
}
