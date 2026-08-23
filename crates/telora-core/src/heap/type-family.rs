fn copy_roots(
    target: &mut Heap,
    source: HeapView<'_>,
    roots: &[Val],
) -> Result<Vec<Val>, HeapError> {
    let mut pending = PendingCopy::new(target, &source);
    let roots = roots
        .iter()
        .map(|root| pending.copy_value(target, &source, *root))
        .collect::<Result<Vec<_>, _>>()?;
    pending.validate()?;
    pending.commit(target);
    Ok(roots)
}

pub(crate) fn instantiate_type_family(
    target: &mut Heap,
    background: Option<&Heap>,
    template: Val,
    arguments: &[Val],
    argument_descriptors: &[crate::types::TypeDescriptor],
) -> Result<(Val, usize), HeapError> {
    let (root, pending) = {
        let source = HeapView {
            current: target,
            background,
        };
        let (replacements, forced_objects) = bound_type_replacements(&source, template, arguments)?;
        let mut pending = PendingCopy::new_type_application(
            target,
            &source,
            replacements,
            forced_objects,
            arguments,
            argument_descriptors,
        );
        let root = pending.copy_value(target, &source, template)?;
        pending.validate()?;
        (root, pending)
    };
    let allocation_count = pending.objects.len();
    pending.commit(target);
    Ok((root, allocation_count))
}

fn bound_type_replacements(
    source: &HeapView<'_>,
    root: Val,
    arguments: &[Val],
) -> Result<(HashMap<Handle, Val>, HashSet<Handle>), HeapError> {
    let mut replacements = HashMap::new();
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    let mut parents = HashMap::<Handle, Vec<Handle>>::new();
    let mut forced_objects = HashSet::new();
    while let Some(value) = pending.pop() {
        let Some(handle) = runtime_object_handle(value.value()) else {
            continue;
        };
        if !visited.insert(handle) {
            continue;
        }
        let object = source.object(handle)?;
        if let Object::DeclaredType { id, .. } | Object::SymbolicType { id, .. } = object
            && id
                .arguments()
                .iter()
                .any(crate::types::type_identity_contains_bound_parameter)
        {
            // Nominal identity retains phantom arguments even when the
            // structural body contains no corresponding Bound metadata.
            forced_objects.insert(handle);
        }
        if let Object::Dict { shape, values } = object {
            let fields = source.shape(*shape)?;
            let mut kind = None;
            let mut parameter = None;
            for (field, value) in fields.iter().zip(values.iter()) {
                match source.text(*field)? {
                    "kind" => kind = source.atom_text(*value)?,
                    "parameter" => {
                        if let DecodedValue::Int(index) = value.value() {
                            parameter = usize::try_from(index).ok();
                        }
                    }
                    _ => {}
                }
            }
            if kind.is_some_and(|kind| kind == "Bound") {
                let index = parameter.ok_or(HeapError("Bound metadata has no parameter index"))?;
                let argument = arguments
                    .get(index)
                    .copied()
                    .ok_or(HeapError("Bound metadata parameter is out of range"))?;
                replacements.insert(handle, argument);
                forced_objects.insert(handle);
                continue;
            }
        }
        let children = match object {
            Object::DeclaredType {
                body, sealed: true, ..
            }
            | Object::SymbolicType {
                body, sealed: true, ..
            } => vec![*body],
            Object::DeclaredType { sealed: false, .. }
            | Object::SymbolicType { sealed: false, .. } => {
                return Err(HeapError("type ref is not sealed"));
            }
            Object::Array(values) | Object::Tuple(values) => values.to_vec(),
            Object::Tagged { tag, payload } => vec![*tag, *payload],
            Object::Dict { values, .. } => values.to_vec(),
            Object::Module { exports } => exports.values.to_vec(),
            Object::Closure { upvalues, .. } => upvalues.to_vec(),
            Object::Dyn {
                descriptor, value, ..
            } => vec![*descriptor, *value],
            Object::TypeSlot { value } => {
                vec![value.ok_or(HeapError("uninitialized type metadata up-link"))?]
            }
            Object::ByteCodeProto { values, .. } => values.to_vec(),
            Object::OpenFunc => return Err(HeapError("function ref is not sealed")),
            Object::Reserved | Object::Bytes(_) | Object::Opaque(_) => Vec::new(),
        };
        for child in children {
            if let Some(child_handle) = runtime_object_handle(child.value()) {
                parents.entry(child_handle).or_default().push(handle);
            }
            pending.push(child);
        }
    }
    let mut affected = forced_objects.iter().copied().collect::<Vec<_>>();
    while let Some(child) = affected.pop() {
        for parent in parents.get(&child).into_iter().flatten() {
            if forced_objects.insert(*parent) {
                affected.push(*parent);
            }
        }
    }
    Ok((replacements, forced_objects))
}

fn runtime_object_handle(value: DecodedValue) -> Option<Handle> {
    match value {
        DecodedValue::NativeType(_) => None,
        DecodedValue::Bytes(handle)
        | DecodedValue::DeclaredType(handle)
        | DecodedValue::SymbolicType(handle)
        | DecodedValue::Opaque(handle)
        | DecodedValue::Array(handle)
        | DecodedValue::Tuple(handle)
        | DecodedValue::Tagged(handle)
        | DecodedValue::Dict(handle)
        | DecodedValue::Module(handle)
        | DecodedValue::Func(handle)
        | DecodedValue::Dyn(handle)
        | DecodedValue::TypeSlot(handle) => Some(handle),
        DecodedValue::Failed(_)
        | DecodedValue::Int(_)
        | DecodedValue::Float(_)
        | DecodedValue::BuiltinAtom(_)
        | DecodedValue::InlineAtom(_)
        | DecodedValue::Atom(_)
        | DecodedValue::InlineString(_)
        | DecodedValue::ShortString(_)
        | DecodedValue::FuncRef(_) => None,
    }
}

