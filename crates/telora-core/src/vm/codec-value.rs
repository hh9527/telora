#[derive(Clone, Debug)]
enum CodecNode {
    Existing(Val),
    Declared {
        owner: Val,
        payload: Box<Self>,
        loc: Option<crate::Loc>,
    },
    Atom(BuiltinAtom, Option<crate::Loc>),
    NamedAtom(String, Option<crate::Loc>),
    Array(Vec<Self>, Option<crate::Loc>),
    Tuple(Vec<Self>, Option<crate::Loc>),
    Tagged {
        tag: Box<Self>,
        payload: Box<Self>,
        loc: Option<crate::Loc>,
    },
    Dict(Vec<(String, Self)>, Option<crate::Loc>),
    String(String, Option<crate::Loc>),
}

#[derive(Clone, Debug)]
struct CodecFailure {
    message: String,
    input: Option<Val>,
}

fn decode_blame(
    message: String,
    subjects: Vec<Val>,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    account: &mut QuotaAccount,
) -> Result<Val, RuntimeError> {
    let bytes = logical_value_bytes(subjects.len().saturating_add(3))
        .and_then(|bytes| {
            bytes
                .checked_add(message.len() as u64)
                .ok_or_else(|| NativeError::allocation_limit("decode error size overflowed"))
        })
        .map_err(|err| allocation_error(err.message, function, pc))?;
    charge_allocation(account, bytes, function, pc)?;
    let location = subjects.first().and_then(|value| value.loc());
    let mut opaque = crate::OpaqueValue::new_identity(crate::core::blame_native_type(), message);
    opaque.traced = subjects.into_boxed_slice();
    Ok(Val::new(
        DecodedValue::Opaque(current.allocate(Object::Opaque(opaque))),
        location,
    ))
}

fn codec_node_bytes(
    node: &CodecNode,
    current: &Heap,
    background: &Heap,
) -> Result<u64, NativeError> {
    match node {
        CodecNode::Existing(_) | CodecNode::Atom(_, _) => Ok(0),
        CodecNode::Declared { payload, .. } => codec_node_bytes(payload, current, background),
        CodecNode::NamedAtom(value, _) | CodecNode::String(value, _) => Ok(value.len() as u64),
        CodecNode::Array(items, _) | CodecNode::Tuple(items, _) => {
            let own = logical_value_bytes(items.len())?;
            items.iter().try_fold(own, |total, item| {
                total
                    .checked_add(codec_node_bytes(item, current, background)?)
                    .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed"))
            })
        }
        CodecNode::Tagged { tag, payload, .. } => {
            let tag = codec_node_bytes(tag, current, background)?;
            let payload = codec_node_bytes(payload, current, background)?;
            logical_value_bytes(2)?
                .checked_add(tag)
                .and_then(|total| total.checked_add(payload))
                .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed"))
        }
        CodecNode::Dict(fields, _) => {
            let own = logical_value_bytes(fields.len())?;
            fields.iter().try_fold(own, |total, (name, value)| {
                let value_bytes = codec_node_bytes(value, current, background)?;
                total
                    .checked_add(name.len() as u64)
                    .and_then(|total| total.checked_add(value_bytes))
                    .ok_or_else(|| NativeError::allocation_limit("codec output size overflowed"))
            })
        }
    }
}

fn materialize_codec_node(node: CodecNode, current: &mut Heap, background: &Heap) -> Val {
    match node {
        CodecNode::Existing(value) => value,
        CodecNode::Declared {
            owner,
            payload,
            loc,
        } => {
            let payload = materialize_codec_node(*payload, current, background);
            let DecodedValue::SolvedType(id) = owner.value() else {
                unreachable!("diagnostic metadata was validated before allocation")
            };
            let type_id = crate::TypeId::solved(id);
            payload.with_type_id(type_id).with_loc(loc)
        }
        CodecNode::Atom(atom, loc) => Val::new(DecodedValue::BuiltinAtom(atom), loc),
        CodecNode::NamedAtom(value, loc) => Val::new(current.atom(Some(background), &value), loc),
        CodecNode::String(value, loc) => Val::new(current.string(Some(background), &value), loc),
        CodecNode::Array(items, loc) => {
            let items = items
                .into_iter()
                .map(|item| materialize_codec_node(item, current, background))
                .collect::<Box<_>>();
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(items))),
                loc,
            )
        }
        CodecNode::Tuple(items, loc) => {
            let items = items
                .into_iter()
                .map(|item| materialize_codec_node(item, current, background))
                .collect::<Box<_>>();
            Val::new(
                DecodedValue::Tuple(current.allocate(Object::Tuple(items))),
                loc,
            )
        }
        CodecNode::Tagged { tag, payload, loc } => {
            let tag = materialize_codec_node(*tag, current, background);
            let payload = materialize_codec_node(*payload, current, background);
            Val::new(
                DecodedValue::Tagged(current.allocate(Object::Tagged { tag, payload })),
                loc,
            )
        }
        CodecNode::Dict(fields, loc) => {
            let mut fields = fields;
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            let (fields, values): (Vec<_>, Vec<_>) = fields
                .into_iter()
                .map(|(name, value)| {
                    (
                        current.intern(&name),
                        materialize_codec_node(value, current, background),
                    )
                })
                .unzip();
            let shape = current.intern_shape(fields);
            Val::new(
                DecodedValue::Dict(current.allocate(Object::Dict {
                    shape,
                    values: values.into(),
                })),
                loc,
            )
        }
    }
}
#[allow(clippy::too_many_arguments)]
fn finish_decode_failure(
    failure: CodecFailure,
    input: Val,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let value = failure.input.unwrap_or(input);
    let bytes = logical_value_bytes(4)
        .and_then(|bytes| {
            bytes
                .checked_add(failure.message.len() as u64)
                .ok_or_else(|| NativeError::allocation_limit("decode error size overflowed"))
        })
        .map_err(|native_error| allocation_error(native_error.message, function, pc))?;
    charge_allocation(account, bytes, function, pc)?;
    let mut opaque =
        crate::OpaqueValue::new_identity(crate::core::blame_native_type(), failure.message);
    opaque.traced = vec![value].into_boxed_slice();
    let blame = Val::new(
        DecodedValue::Opaque(current.allocate(Object::Opaque(opaque))),
        value.loc(),
    );
    finish_codec_payload(
        BuiltinAtom::Err,
        CodecNode::Existing(blame),
        input,
        return_target,
        function,
        pc,
        current,
        background,
        account,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_codec_payload(
    tag: BuiltinAtom,
    payload: CodecNode,
    input: Val,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let bytes = codec_node_bytes(&payload, current, background)
        .and_then(|bytes| {
            bytes
                .checked_add(logical_value_bytes(2)?)
                .ok_or_else(|| NativeError::allocation_limit("codec Result size overflowed"))
        })
        .map_err(|native_error| match native_error.limit() {
            Some(_) => allocation_error(native_error.message, function, pc),
            None => error(
                RuntimeErrorKind::TypeMismatch,
                native_error.message,
                function,
                pc,
            ),
        })?;
    charge_allocation(account, bytes, function, pc)?;
    let payload = materialize_codec_node(payload, current, background);
    let value = Val::new(
        DecodedValue::Tagged(current.allocate(Object::Tagged {
            tag: Val::new(DecodedValue::BuiltinAtom(tag), input.loc()),
            payload,
        })),
        input.loc(),
    );
    Ok(VmAction::Return {
        value,
        return_target,
    })
}
