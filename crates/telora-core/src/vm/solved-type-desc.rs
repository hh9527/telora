/// Type metadata is an observation of the immutable session arena. Even nominal
/// bodies already have IDs; resolve never allocates or infers a type.
fn run_solved_type_desc(
    operation: CoreTypeDescFunction,
    arguments: &[Val],
    signature: Option<Val>,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    use crate::mir::TypeConstructor as T;
    let types = background
        .solved_types
        .as_ref()
        .expect("solved metadata image");
    let input = arguments[0];
    let ty = solved_metadata_id(input, types, function, pc)?;
    let shape = &types.types[ty.index()];
    let signature = signature.ok_or_else(|| {
        error(
            RuntimeErrorKind::InvalidBytecode,
            "TypeDesc requires a compiled native signature",
            function,
            pc,
        )
    })?;
    let signature = solved_metadata_id(signature, types, function, pc)?;
    let signature = &types.types[signature.index()];
    if signature.constructor != T::Function || signature.arguments.len() != 2 {
        return Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "invalid TypeDesc native signature",
            function,
            pc,
        ));
    }
    let result_type = signature.arguments[1];
    let metadata = |id| Val::new(DecodedValue::SolvedType(id), input.loc());
    let value = match operation {
        CoreTypeDescFunction::Kind => {
            let kind = match &shape.constructor {
                T::Never => "Never",
                T::Type | T::Meta => "Type",
                T::TypeOf => "TypeOf",
                T::Int => "Int",
                T::Float => "Float",
                T::String => "String",
                T::Bytes => "Bytes",
                T::Array => "Array",
                T::Dict => "Dict",
                T::Tuple => "Tuple",
                T::Record(_) => "Struct",
                T::Newtype => "Newtype",
                T::Enum(_) | T::Bool | T::Option | T::Result | T::FoldControl => "Enum",
                T::Function => "Func",
                T::Native(_) => "Opaque",
                T::Parameter(_) | T::PropertyBound => "Bound",
                T::Dyn => "Dyn",
                T::Nominal(_) | T::Unchecked => "Ref",
                _ => {
                    return Err(error(
                        RuntimeErrorKind::InvalidBytecode,
                        "unsupported solved metadata constructor",
                        function,
                        pc,
                    ));
                }
            };
            Val::new(current.atom(Some(background), kind), input.loc())
                .with_type_id(crate::TypeId::solved(result_type))
        }
        CoreTypeDescFunction::Resolve => {
            if let Some(layout) = types.layout(ty) {
                return finish_codec_payload(
                    BuiltinAtom::Ok,
                    CodecNode::Existing(metadata(layout.body)),
                    input,
                    return_target,
                    function,
                    pc,
                    current,
                    background,
                    account,
                );
            }
            return finish_codec_payload(
                BuiltinAtom::Err,
                CodecNode::String(
                    "type descriptor is not a recursive reference".into(),
                    input.loc(),
                ),
                input,
                return_target,
                function,
                pc,
                current,
                background,
                account,
            );
        }
        CoreTypeDescFunction::Children => {
            let children = match &shape.constructor {
                T::TypeOf
                | T::Array
                | T::Dict
                | T::Tuple
                | T::Record(_)
                | T::Newtype
                | T::Enum(_)
                | T::Option
                | T::Result
                | T::FoldControl => shape
                    .arguments
                    .iter()
                    .copied()
                    .map(metadata)
                    .collect::<Vec<_>>(),
                _ => vec![],
            };
            charge_allocation(
                account,
                logical_value_bytes(children.len())
                    .map_err(|e| allocation_error(e.message, function, pc))?,
                function,
                pc,
            )?;
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(children.into()))),
                input.loc(),
            )
        }
        CoreTypeDescFunction::Fields | CoreTypeDescFunction::Variants => {
            let variants = operation == CoreTypeDescFunction::Variants;
            let members = solved_type_desc_members(ty, variants, types)
                .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
            let result = &types.types[result_type.index()];
            if result.constructor != T::Array || result.arguments.len() != 1 {
                return Err(error(
                    RuntimeErrorKind::InvalidBytecode,
                    "metadata member signature does not return Array",
                    function,
                    pc,
                ));
            }
            let member_type = result.arguments[0];
            let mut output = vec![];
            charge_allocation(
                account,
                logical_value_bytes(2 + members.len() * 6)
                    .map_err(|e| allocation_error(e.message, function, pc))?,
                function,
                pc,
            )?;
            for (index, (name, child)) in members.into_iter().enumerate() {
                charge_allocation(account, name.len() as u64, function, pc)?;
                let child = if variants {
                    if let Some(child) = child {
                        solved_some(metadata(child), current, account, function, pc)?
                    } else {
                        Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), input.loc())
                    }
                } else {
                    metadata(child.expect("Struct member"))
                };
                let name = Val::new(current.string(Some(background), &name), input.loc());
                let value = current
                    .record_value([
                        (
                            "index".into(),
                            Val::new(DecodedValue::Int(index as i64), input.loc()),
                        ),
                        ("name".into(), name),
                        ((if variants { "payload" } else { "ty" }).into(), child),
                    ])
                    .map_err(|e| {
                        error(
                            RuntimeErrorKind::InvalidBytecode,
                            e.to_string(),
                            function,
                            pc,
                        )
                    })?;
                output.push(
                    value
                        .with_loc(input.loc())
                        .with_type_id(crate::TypeId::solved(member_type)),
                );
            }
            Val::new(
                DecodedValue::Array(current.allocate(Object::Array(output.into()))),
                input.loc(),
            )
        }
        CoreTypeDescFunction::OpaqueName => {
            if let T::Native(native) = shape.constructor {
                let name = types
                    .native_definitions
                    .iter()
                    .find(|(id, _)| *id == native)
                    .map(|(_, name)| name)
                    .ok_or_else(|| {
                        error(
                            RuntimeErrorKind::InvalidBytecode,
                            "native metadata has no source identity",
                            function,
                            pc,
                        )
                    })?;
                charge_allocation(account, name.len() as u64, function, pc)?;
                let name = Val::new(current.string(Some(background), name), input.loc());
                solved_some(name, current, account, function, pc)?
            } else {
                Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), input.loc())
            }
        }
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

fn solved_type_desc_members(
    ty: crate::mir::TypeId,
    variants: bool,
    types: &crate::type_image::TypeImage,
) -> Result<Vec<(String, Option<crate::mir::TypeId>)>, String> {
    use crate::mir::TypeConstructor as T;
    let ty = types.layout(ty).map_or(ty, |layout| layout.body);
    let shape = &types.types[ty.index()];
    match (&shape.constructor, variants) {
        (T::Record(names), false) => Ok(names
            .iter()
            .cloned()
            .zip(shape.arguments.iter().copied().map(Some))
            .collect()),
        (T::Enum(names), true) => {
            let mut arguments = shape.arguments.iter().copied();
            Ok(names
                .iter()
                .map(|(name, payload)| {
                    (
                        name.clone(),
                        if *payload {
                            Some(arguments.next().expect("enum body argument"))
                        } else {
                            None
                        },
                    )
                })
                .collect())
        }
        (T::Bool, true) => Ok(vec![("False".into(), None), ("True".into(), None)]),
        (T::Option | T::Result | T::FoldControl, true) => Ok((0..2)
            .map(|index| {
                let (name, payload) = crate::type_image::builtin_variant(&shape.constructor, index)
                    .expect("builtin variant");
                (
                    name.into(),
                    payload.then(|| {
                        shape.arguments[if shape.constructor == T::Option {
                            0
                        } else {
                            index as usize
                        }]
                    }),
                )
            })
            .collect()),
        _ => Err(format!(
            "std/type-desc.{} expects {}",
            if variants { "variants" } else { "fields" },
            if variants { "Enum" } else { "Struct" }
        )),
    }
}
