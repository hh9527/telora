/// Converts the private raw graph produced by a data frontend into the public
/// recursively tagged `std/value.Value` graph. Variant wrappers retain the raw
/// node location; provenance paths therefore continue to address semantic
/// array indices and object keys rather than implementation wrappers.
pub(crate) fn wrap_semantic_value(
    current: &mut Heap,
    background: Option<&Heap>,
    raw: Val,
    owner: Val,
) -> Result<Val, HeapError> {
    enum RawNode {
        Unit(BuiltinAtom),
        Scalar(&'static str, Val),
        Array(Vec<Val>),
        Object(Vec<(String, Val)>),
        Temporal(String, Val),
    }

    let view = HeapView {
        current,
        background,
    };
    let type_id = view.declared_type_id(owner)?;
    let node = match raw.value() {
        DecodedValue::BuiltinAtom(
            atom @ (BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False),
        ) => RawNode::Unit(atom),
        DecodedValue::Int(_) => RawNode::Scalar("Int", raw.without_type_id()),
        DecodedValue::Float(value) if value.is_finite() => {
            RawNode::Scalar("Float", raw.without_type_id())
        }
        DecodedValue::Float(_) => {
            return Err(HeapError(
                "semantic Value cannot contain a non-finite Float",
            ));
        }
        DecodedValue::InlineString(_) | DecodedValue::ShortString(_) => {
            RawNode::Scalar("String", raw.without_type_id())
        }
        DecodedValue::Bytes(_) => RawNode::Scalar("Bytes", raw.without_type_id()),
        DecodedValue::Array(handle) => RawNode::Array(view.sequence(handle, false)?.to_vec()),
        DecodedValue::Dict(handle) => {
            let (fields, values) = view.dict_parts(handle)?;
            let fields = fields
                .iter()
                .map(|field| view.text(*field).map(str::to_owned))
                .collect::<Result<Vec<_>, _>>()?;
            RawNode::Object(fields.into_iter().zip(values.iter().copied()).collect())
        }
        DecodedValue::Tagged(handle) => {
            let (tag, payload) = view.tagged(handle)?;
            let tag = view
                .atom_text(tag)?
                .ok_or(HeapError("semantic temporal tag is not an Atom"))?
                .as_str()
                .to_owned();
            if !matches!(
                tag.as_str(),
                "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
            ) {
                return Err(HeapError::owned(format!(
                    "raw data graph contains unsupported tag {tag:?}"
                )));
            }
            if view.string_text(payload)?.is_none() {
                return Err(HeapError("semantic temporal payload is not a String"));
            }
            RawNode::Temporal(tag, payload.without_type_id())
        }
        DecodedValue::NativeType(_)
        | DecodedValue::DeclaredType(_)
        | DecodedValue::SymbolicType(_)
        | DecodedValue::TypeSlot(_) => {
            return Err(HeapError("semantic Value cannot encode Type"));
        }
        _ => {
            return Err(HeapError::owned(format!(
                "raw data graph contains unsupported {:?}",
                raw.value()
            )));
        }
    };

    let loc = raw.loc();
    let value = match node {
        RawNode::Unit(atom) => Val::new(DecodedValue::BuiltinAtom(atom), loc),
        RawNode::Scalar(tag, payload) => {
            let tag = Val::new(current.atom(background, tag), loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
        RawNode::Array(items) => {
            let items = items
                .into_iter()
                .map(|item| wrap_semantic_value(current, background, item, owner))
                .collect::<Result<Box<[_]>, _>>()?;
            let payload = Val::new(
                DecodedValue::Array(current.allocate(Object::Array(items))),
                loc,
            );
            let tag = Val::new(current.atom(background, "Array"), loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
        RawNode::Object(fields) => {
            let mut fields = fields
                .into_iter()
                .map(|(name, value)| {
                    wrap_semantic_value(current, background, value, owner)
                        .map(|value| (name, value))
                })
                .collect::<Result<Vec<_>, _>>()?;
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            let (names, values): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .map(|(name, value)| (current.intern(&name), value))
                .unzip();
            let shape = current.intern_shape(names);
            let payload = Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: values.into(),
                })),
                loc,
            );
            let tag = Val::new(current.atom(background, "Object"), loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
        RawNode::Temporal(tag, payload) => {
            let tag = Val::new(current.atom(background, &tag), loc);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
    };
    Ok(value.with_type_id(type_id))
}

pub(crate) fn semantic_value_type_id(
    current: &Heap,
    background: Option<&Heap>,
    owner: Val,
) -> Result<crate::TypeId, HeapError> {
    HeapView {
        current,
        background,
    }
    .declared_type_id(owner)
}

pub(crate) fn semantic_value_wrapper_bytes(
    current: &Heap,
    background: Option<&Heap>,
    raw: Val,
) -> Result<u64, HeapError> {
    fn add(left: u64, right: u64) -> Result<u64, HeapError> {
        left.checked_add(right)
            .ok_or(HeapError("semantic Value size overflowed"))
    }

    fn visit(
        view: &HeapView<'_>,
        raw: Val,
        active: &mut HashSet<Handle>,
    ) -> Result<u64, HeapError> {
        let tagged_bytes = (std::mem::size_of::<Val>() as u64)
            .checked_mul(2)
            .ok_or(HeapError("semantic Value size overflowed"))?;
        match raw.value() {
            DecodedValue::BuiltinAtom(
                BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False,
            ) => Ok(0),
            DecodedValue::Int(_)
            | DecodedValue::InlineString(_)
            | DecodedValue::ShortString(_)
            | DecodedValue::Bytes(_) => Ok(tagged_bytes),
            DecodedValue::Float(value) if value.is_finite() => Ok(tagged_bytes),
            DecodedValue::Float(_) => Err(HeapError(
                "semantic Value cannot contain a non-finite Float",
            )),
            DecodedValue::Array(handle) => {
                if !active.insert(handle) {
                    return Err(HeapError("semantic Value cannot contain a cycle"));
                }
                let items = view.sequence(handle, false)?;
                let own = (std::mem::size_of::<Val>() as u64)
                    .checked_mul(items.len() as u64)
                    .ok_or(HeapError("semantic Value size overflowed"))?;
                let mut bytes = add(own, tagged_bytes)?;
                for item in items {
                    bytes = add(bytes, visit(view, *item, active)?)?;
                }
                active.remove(&handle);
                Ok(bytes)
            }
            DecodedValue::Dict(handle) => {
                if !active.insert(handle) {
                    return Err(HeapError("semantic Value cannot contain a cycle"));
                }
                let (_, values) = view.dict_parts(handle)?;
                let own = (std::mem::size_of::<Val>() as u64)
                    .checked_mul(values.len() as u64)
                    .ok_or(HeapError("semantic Value size overflowed"))?;
                let mut bytes = add(own, tagged_bytes)?;
                for value in values {
                    bytes = add(bytes, visit(view, *value, active)?)?;
                }
                active.remove(&handle);
                Ok(bytes)
            }
            DecodedValue::Tagged(handle) => {
                if !active.insert(handle) {
                    return Err(HeapError("semantic Value cannot contain a cycle"));
                }
                let (tag, payload) = view.tagged(handle)?;
                let tag = view
                    .atom_text(tag)?
                    .ok_or(HeapError("semantic temporal tag is not an Atom"))?;
                if !matches!(
                    tag.as_str(),
                    "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
                ) || view.string_text(payload)?.is_none()
                {
                    return Err(HeapError(
                        "raw data graph contains unsupported tagged value",
                    ));
                }
                active.remove(&handle);
                Ok(tagged_bytes)
            }
            DecodedValue::NativeType(_)
            | DecodedValue::DeclaredType(_)
            | DecodedValue::SymbolicType(_)
            | DecodedValue::TypeSlot(_) => Err(HeapError("semantic Value cannot encode Type")),
            _ => Err(HeapError::owned(format!(
                "raw data graph contains unsupported {:?}",
                raw.value()
            ))),
        }
    }

    visit(
        &HeapView {
            current,
            background,
        },
        raw,
        &mut HashSet::new(),
    )
}

pub(crate) fn semantic_value_unwrap_bytes(
    current: &Heap,
    background: Option<&Heap>,
    value: Val,
    owner: Val,
) -> Result<u64, HeapError> {
    fn add(left: u64, right: u64) -> Result<u64, HeapError> {
        left.checked_add(right)
            .ok_or(HeapError("semantic Value size overflowed"))
    }

    fn visit(
        view: &HeapView<'_>,
        value: Val,
        expected: crate::TypeId,
        active: &mut HashSet<Handle>,
    ) -> Result<u64, HeapError> {
        if value.type_id() != Some(expected) {
            return Err(HeapError(
                "data value does not have the canonical std/value.Value identity",
            ));
        }
        match value.value() {
            DecodedValue::BuiltinAtom(
                BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False,
            ) => Ok(0),
            DecodedValue::Tagged(handle) => {
                if !active.insert(handle) {
                    return Err(HeapError("std/value.Value cannot contain a cycle"));
                }
                let (tag, payload) = view.tagged(handle)?;
                let tag = view
                    .atom_text(tag)?
                    .ok_or(HeapError("Value variant tag is not an Atom"))?;
                let bytes = match tag.as_str() {
                    "Int" if matches!(payload.value(), DecodedValue::Int(_)) => 0,
                    "Float" if matches!(payload.value(), DecodedValue::Float(value) if value.is_finite()) => {
                        0
                    }
                    "String" if view.string_text(payload)?.is_some() => 0,
                    "Bytes" if matches!(payload.value(), DecodedValue::Bytes(_)) => 0,
                    "Array" => {
                        let DecodedValue::Array(payload_handle) = payload.value() else {
                            return Err(HeapError("Value.Array payload is not an Array"));
                        };
                        let items = view.sequence(payload_handle, false)?;
                        let mut bytes = (std::mem::size_of::<Val>() as u64)
                            .checked_mul(items.len() as u64)
                            .ok_or(HeapError("semantic Value size overflowed"))?;
                        for item in items {
                            bytes = add(bytes, visit(view, *item, expected, active)?)?;
                        }
                        bytes
                    }
                    "Object" => {
                        let DecodedValue::Dict(payload_handle) = payload.value() else {
                            return Err(HeapError("Value.Object payload is not a Dict"));
                        };
                        let (_, values) = view.dict_parts(payload_handle)?;
                        let mut bytes = (std::mem::size_of::<Val>() as u64)
                            .checked_mul(values.len() as u64)
                            .ok_or(HeapError("semantic Value size overflowed"))?;
                        for value in values {
                            bytes = add(bytes, visit(view, *value, expected, active)?)?;
                        }
                        bytes
                    }
                    "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
                        if view.string_text(payload)?.is_some() =>
                    {
                        std::mem::size_of::<Val>() as u64
                    }
                    _ => {
                        return Err(HeapError::owned(format!(
                            "invalid std/value.Value variant {:?}",
                            tag.as_str()
                        )));
                    }
                };
                active.remove(&handle);
                Ok(bytes)
            }
            _ => Err(HeapError(
                "std/value.Value has an invalid runtime representation",
            )),
        }
    }

    let view = HeapView {
        current,
        background,
    };
    let expected = view.declared_type_id(owner)?;
    visit(&view, value, expected, &mut HashSet::new())
}

/// Removes the public `Value` variants into the private raw graph consumed by
/// the existing schema transformer and format writers.
pub(crate) fn unwrap_semantic_value(
    current: &mut Heap,
    background: Option<&Heap>,
    value: Val,
    owner: Val,
) -> Result<Val, HeapError> {
    enum ValueNode {
        Unit(BuiltinAtom),
        Scalar(Val),
        Array(Vec<Val>),
        Object(Vec<(String, Val)>),
        Temporal(String, Val),
    }

    let view = HeapView {
        current,
        background,
    };
    let expected = view.declared_type_id(owner)?;
    if value.type_id() != Some(expected) {
        return Err(HeapError(
            "data value does not have the canonical std/value.Value identity",
        ));
    }
    let node = match value.value() {
        DecodedValue::BuiltinAtom(
            atom @ (BuiltinAtom::None | BuiltinAtom::True | BuiltinAtom::False),
        ) => ValueNode::Unit(atom),
        DecodedValue::Tagged(handle) => {
            let (tag, payload) = view.tagged(handle)?;
            let tag = view
                .atom_text(tag)?
                .ok_or(HeapError("Value variant tag is not an Atom"))?
                .as_str()
                .to_owned();
            match tag.as_str() {
                "Int" if matches!(payload.value(), DecodedValue::Int(_)) => {
                    ValueNode::Scalar(payload)
                }
                "Float" if matches!(payload.value(), DecodedValue::Float(value) if value.is_finite()) => {
                    ValueNode::Scalar(payload)
                }
                "String" if view.string_text(payload)?.is_some() => ValueNode::Scalar(payload),
                "Bytes" if matches!(payload.value(), DecodedValue::Bytes(_)) => {
                    ValueNode::Scalar(payload)
                }
                "Array" => {
                    let DecodedValue::Array(handle) = payload.value() else {
                        return Err(HeapError("Value.Array payload is not an Array"));
                    };
                    ValueNode::Array(view.sequence(handle, false)?.to_vec())
                }
                "Object" => {
                    let DecodedValue::Dict(handle) = payload.value() else {
                        return Err(HeapError("Value.Object payload is not a Dict"));
                    };
                    let (fields, values) = view.dict_parts(handle)?;
                    let fields = fields
                        .iter()
                        .map(|field| view.text(*field).map(str::to_owned))
                        .collect::<Result<Vec<_>, _>>()?;
                    ValueNode::Object(fields.into_iter().zip(values.iter().copied()).collect())
                }
                "LocalDate" | "LocalTime" | "LocalDateTime" | "OffsetDateTime"
                    if view.string_text(payload)?.is_some() =>
                {
                    ValueNode::Temporal(tag, payload)
                }
                _ => {
                    return Err(HeapError::owned(format!(
                        "invalid std/value.Value variant {tag:?}"
                    )));
                }
            }
        }
        _ => {
            return Err(HeapError(
                "std/value.Value has an invalid runtime representation",
            ));
        }
    };

    let loc = value.loc();
    match node {
        ValueNode::Unit(atom) => Ok(Val::new(DecodedValue::BuiltinAtom(atom), loc)),
        ValueNode::Scalar(payload) => Ok(payload.without_type_id().with_loc(payload.loc().or(loc))),
        ValueNode::Array(items) => {
            let items = items
                .into_iter()
                .map(|item| unwrap_semantic_value(current, background, item, owner))
                .collect::<Result<Box<[_]>, _>>()?;
            Ok(Val::new(
                DecodedValue::Array(current.allocate(Object::Array(items))),
                loc,
            ))
        }
        ValueNode::Object(fields) => {
            let mut fields = fields
                .into_iter()
                .map(|(name, value)| {
                    unwrap_semantic_value(current, background, value, owner)
                        .map(|value| (name, value))
                })
                .collect::<Result<Vec<_>, _>>()?;
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            let (names, values): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .map(|(name, value)| (current.intern(&name), value))
                .unzip();
            let shape = current.intern_shape(names);
            Ok(Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: values.into(),
                })),
                loc,
            ))
        }
        ValueNode::Temporal(tag, payload) => {
            let field = current.intern(&tag);
            let shape = current.intern_shape(vec![field]);
            Ok(Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: Box::new([payload]),
                })),
                loc,
            ))
        }
    }
}
