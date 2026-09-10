fn solved_metadata_id(
    value: Val,
    types: &crate::type_image::TypeImage,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<crate::mir::TypeId, RuntimeError> {
    match value.value() {
        DecodedValue::SolvedType(id) if id.index() < types.types.len() => Ok(id),
        _ => Err(error(
            RuntimeErrorKind::InvalidBytecode,
            "solved Dyn operation requires a TypeId from its session image",
            function,
            pc,
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_solved_dyn(
    operation: CoreDynFunction,
    arguments: &[Val],
    signature: Option<Val>,
    return_target: ReturnTarget,
    function: &BytecodeFunction,
    pc: usize,
    current: &mut Heap,
    background: &Heap,
    account: &mut QuotaAccount,
) -> Result<VmAction, RuntimeError> {
    let types = background
        .solved_types
        .as_ref()
        .expect("solved Dyn session");
    if operation == CoreDynFunction::Pack {
        solved_metadata_id(arguments[0], types, function, pc)?;
        // The generic signature proves the pairing before codegen. The VM stores
        // that witness and the original Val, without inspecting/copying its graph.
        let payload = arguments[1];
        propagate_direct_failure(&payload, function, pc)?;
        charge_allocation(
            account,
            logical_value_bytes(2).map_err(|e| allocation_error(e.message, function, pc))?,
            function,
            pc,
        )?;
        let handle = current.allocate(Object::Dyn {
            identity: Arc::new(()),
            descriptor: arguments[0],
            value: payload,
            scheme: None,
            origin: None,
        });
        return Ok(VmAction::Return {
            value: Val::new(DecodedValue::Dyn(handle), payload.loc()),
            return_target,
        });
    }
    let input = arguments[usize::from(operation == CoreDynFunction::ProjectWith)];
    propagate_direct_failure(&input, function, pc)?;
    let DecodedValue::Dyn(handle) = input.value() else {
        return Err(runtime_shallow_type_error("Dyn", input, function, pc));
    };
    let view = HeapView {
        current,
        background: Some(background),
    };
    let (_, descriptor, payload) = view
        .dyn_parts(handle)
        .map_err(|e| core_dict_heap_error(e, function, pc))?;
    let packaged = solved_metadata_id(descriptor, types, function, pc)?;
    if operation == CoreDynFunction::GetFieldValue {
        let index = dyn_member_index(arguments[1], function, pc)? as usize;
        let fields = solved_dyn_fields(packaged, types)
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        let (name, ty) = fields.get(index).ok_or_else(|| {
            error(
                RuntimeErrorKind::TypeMismatch,
                "Struct field index is out of range",
                function,
                pc,
            )
        })?;
        let child = (ValueRef {
            value: payload,
            view,
        })
        .dict_get(name)
        .ok_or_else(|| {
            error(
                RuntimeErrorKind::InvalidBytecode,
                "solved Struct payload is missing a field",
                function,
                pc,
            )
        })?
        .value;
        let value = pack_solved_dyn(*ty, child, current, account, function, pc)?;
        return Ok(VmAction::Return {
            value,
            return_target,
        });
    }
    if matches!(
        operation,
        CoreDynFunction::GetVariantIndex | CoreDynFunction::GetVariantPayload
    ) {
        let (index, _, child) = solved_dyn_variant(packaged, payload, types, view)
            .map_err(|message| error(RuntimeErrorKind::TypeMismatch, message, function, pc))?;
        let value = if operation == CoreDynFunction::GetVariantIndex {
            Val::new(DecodedValue::Int(index as i64), payload.loc())
        } else {
            let expected = dyn_member_index(arguments[1], function, pc)? as usize;
            if index != expected {
                return Err(error(
                    RuntimeErrorKind::TypeMismatch,
                    format!("Dyn variant index is {index}, not {expected}"),
                    function,
                    pc,
                ));
            }
            if let Some((ty, child)) = child {
                let child = pack_solved_dyn(ty, child, current, account, function, pc)?;
                solved_some(child, current, account, function, pc)?
            } else {
                Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), payload.loc())
            }
        };
        return Ok(VmAction::Return {
            value,
            return_target,
        });
    }
    if matches!(
        operation,
        CoreDynFunction::Field
            | CoreDynFunction::Fields
            | CoreDynFunction::ArrayItems
            | CoreDynFunction::TupleItems
            | CoreDynFunction::Tag
            | CoreDynFunction::Payload
    ) {
        let field = if operation == CoreDynFunction::Field {
            Some(
                (ValueRef {
                    value: arguments[1],
                    view,
                })
                .as_str()
                .ok_or_else(|| runtime_shallow_type_error("String", arguments[1], function, pc))?
                .as_str()
                .to_owned(),
            )
        } else {
            None
        };
        let observation =
            observe_solved_dyn(operation, packaged, payload, field.as_deref(), types, view);
        return finish_dyn_observation(
            input,
            observation,
            return_target,
            function,
            pc,
            current,
            background,
            account,
        );
    }
    let matches = match operation {
        CoreDynFunction::Kind => {
            use crate::mir::{TypeConstructor as T, TypeOperation};
            let signature = signature.ok_or_else(|| {
                error(
                    RuntimeErrorKind::InvalidBytecode,
                    "Dyn kind requires its compiled native signature",
                    function,
                    pc,
                )
            })?;
            let signature = solved_metadata_id(signature, types, function, pc)?;
            let signature = &types.types[signature.index()];
            if signature.constructor != T::Function || signature.arguments.len() != 2 {
                return Err(error(
                    RuntimeErrorKind::InvalidBytecode,
                    "invalid Dyn kind signature",
                    function,
                    pc,
                ));
            }
            let result_type = signature.arguments[1];
            let kind = match &types.types[packaged.index()].constructor {
                T::Int => "Int",
                T::Float => "Float",
                T::String => "String",
                T::Bytes => "Bytes",
                T::Type | T::TypeOf | T::Meta => "Type",
                T::Native(_) => "Opaque",
                T::Record(_) | T::Dict => "Dict",
                T::Array => "Array",
                T::Tuple => "Tuple",
                T::Function => "Func",
                T::Dyn => "Dyn",
                T::Bool | T::PropertyTarget => "Atom",
                T::Nominal(symbol) => match types
                    .definition(*symbol)
                    .expect("nominal Dyn definition")
                    .operation
                {
                    TypeOperation::Struct => "Dict",
                    TypeOperation::Newtype => "Tuple",
                    TypeOperation::Enum => {
                        if solved_dyn_variant(packaged, payload, types, view)
                            .map_err(|message| {
                                error(RuntimeErrorKind::InvalidBytecode, message, function, pc)
                            })?
                            .2
                            .is_some()
                        {
                            "Tagged"
                        } else {
                            "Atom"
                        }
                    }
                    _ => {
                        return Err(error(
                            RuntimeErrorKind::InvalidBytecode,
                            "invalid Dyn nominal skeleton",
                            function,
                            pc,
                        ));
                    }
                },
                T::Option | T::Result | T::FoldControl => {
                    if solved_dyn_variant(packaged, payload, types, view)
                        .map_err(|message| {
                            error(RuntimeErrorKind::InvalidBytecode, message, function, pc)
                        })?
                        .2
                        .is_some()
                    {
                        "Tagged"
                    } else {
                        "Atom"
                    }
                }
                _ => {
                    return Err(error(
                        RuntimeErrorKind::InvalidBytecode,
                        "Dyn witness is not an executable value type",
                        function,
                        pc,
                    ));
                }
            };
            return Ok(VmAction::Return {
                value: Val::new(current.atom(Some(background), kind), payload.loc())
                    .with_type_id(crate::TypeId::solved(result_type)),
                return_target,
            });
        }
        CoreDynFunction::ProjectWith => {
            solved_metadata_id(arguments[0], types, function, pc)? == packaged
        }
        CoreDynFunction::Desc => {
            return Ok(VmAction::Return {
                value: descriptor,
                return_target,
            });
        }
        CoreDynFunction::CheckInt => {
            types.types[packaged.index()].constructor == crate::mir::TypeConstructor::Int
        }
        CoreDynFunction::CheckFloat => {
            types.types[packaged.index()].constructor == crate::mir::TypeConstructor::Float
        }
        CoreDynFunction::CheckString => {
            types.types[packaged.index()].constructor == crate::mir::TypeConstructor::String
        }
        CoreDynFunction::CheckBytes => {
            types.types[packaged.index()].constructor == crate::mir::TypeConstructor::Bytes
        }
        CoreDynFunction::Pack
        | CoreDynFunction::GetFieldValue
        | CoreDynFunction::GetVariantIndex
        | CoreDynFunction::GetVariantPayload
        | CoreDynFunction::Field
        | CoreDynFunction::Fields
        | CoreDynFunction::ArrayItems
        | CoreDynFunction::TupleItems
        | CoreDynFunction::Tag
        | CoreDynFunction::Payload => unreachable!("handled solved Dyn operation"),
    };
    let value = if matches {
        solved_some(payload, current, account, function, pc)?
    } else {
        Val::new(DecodedValue::BuiltinAtom(BuiltinAtom::None), payload.loc())
    };
    Ok(VmAction::Return {
        value,
        return_target,
    })
}

fn pack_solved_dyn(
    ty: crate::mir::TypeId,
    value: Val,
    current: &mut Heap,
    account: &mut QuotaAccount,
    function: &BytecodeFunction,
    pc: usize,
) -> Result<Val, RuntimeError> {
    charge_allocation(
        account,
        logical_value_bytes(2).map_err(|e| allocation_error(e.message, function, pc))?,
        function,
        pc,
    )?;
    Ok(Val::new(
        DecodedValue::Dyn(current.allocate(Object::Dyn {
            identity: Arc::new(()),
            descriptor: Val::new(DecodedValue::SolvedType(ty), value.loc()),
            value,
            scheme: None,
            origin: None,
        })),
        value.loc(),
    ))
}

/// Declaration indices and applied member identities come from the static image.
fn solved_dyn_fields(
    ty: crate::mir::TypeId,
    types: &crate::type_image::TypeImage,
) -> Result<Vec<(&str, crate::mir::TypeId)>, String> {
    use crate::mir::{TypeConstructor as T, TypeOperation};
    let ty = if types.types[ty.index()].constructor == T::Unchecked {
        types.types[ty.index()].arguments[0]
    } else { ty };
    let shape = &types.types[ty.index()];
    match &shape.constructor {
        T::Record(names) => Ok(names
            .iter()
            .map(String::as_str)
            .zip(shape.arguments.iter().copied())
            .collect()),
        T::Nominal(symbol) => {
            let definition = types
                .definition(*symbol)
                .ok_or("missing nominal definition")?;
            if definition.operation != TypeOperation::Struct {
                return Err("Dyn field access expects Struct".into());
            }
            let layout = types.layout(ty).ok_or("missing applied Struct layout")?;
            Ok(definition
                .members
                .iter()
                .zip(&layout.members)
                .map(|(member, ty)| (member.name.as_str(), ty.expect("Struct member")))
                .collect())
        }
        _ => Err("Dyn field access expects Struct".into()),
    }
}

fn solved_dyn_variant(
    ty: crate::mir::TypeId,
    value: Val,
    types: &crate::type_image::TypeImage,
    view: HeapView<'_>,
) -> Result<(usize, String, Option<(crate::mir::TypeId, Val)>), String> {
    use crate::mir::{TypeConstructor as T, TypeOperation};
    let shape = &types.types[ty.index()];
    let reference = ValueRef { value, view };
    let (tag, payload) = match reference.tagged_parts() {
        Some((tag, payload)) => (tag.as_atom(), Some(payload.value)),
        None => (reference.as_atom(), None),
    };
    let tag = tag.ok_or("Dyn enum payload has no variant tag")?;
    let tag = tag.as_str();
    let (index, member) = match &shape.constructor {
        T::Nominal(symbol) => {
            let definition = types.definition(*symbol).ok_or("missing enum definition")?;
            if definition.operation != TypeOperation::Enum {
                return Err("Dyn variant access expects Enum".into());
            }
            let index = definition
                .members
                .iter()
                .position(|member| member.name == tag)
                .ok_or("unknown enum variant")?;
            (
                index,
                types
                    .layout(ty)
                    .ok_or("missing applied enum layout")?
                    .members[index],
            )
        }
        T::Bool => match tag {
            "False" => (0, None),
            "True" => (1, None),
            _ => return Err("invalid Bool variant".into()),
        },
        T::PropertyTarget => {
            let index = crate::type_image::PROPERTY_TARGET_VARIANTS.iter().position(|name| *name == tag)
                .ok_or("invalid PropertyTarget variant")?;
            (index, None)
        }
        T::Option | T::Result | T::FoldControl => {
            let index = (0..2)
                .find(|&index| {
                    crate::type_image::builtin_variant(&shape.constructor, index)
                        .is_some_and(|(name, _)| name == tag)
                })
                .ok_or("unknown builtin variant")?;
            let member = crate::type_image::builtin_variant_argument(&shape.constructor, index)
                .map(|argument| shape.arguments[argument]);
            (index as usize, member)
        }
        _ => return Err("Dyn variant access expects Enum".into()),
    };
    let child = match (member, payload) {
        (Some(ty), Some(value)) => Some((ty, value)),
        (None, None) => None,
        _ => return Err("enum payload does not match its solved member layout".into()),
    };
    Ok((index, tag.to_owned(), child))
}

fn observe_solved_dyn(
    operation: CoreDynFunction,
    ty: crate::mir::TypeId,
    value: Val,
    field: Option<&str>,
    types: &crate::type_image::TypeImage,
    view: HeapView<'_>,
) -> Result<DynObservation, String> {
    use crate::mir::{TypeConstructor as T, TypeOperation};
    let shape = &types.types[ty.index()];
    let reference = ValueRef { value, view };
    let metadata = |ty| Val::new(DecodedValue::SolvedType(ty), value.loc());
    match operation {
        CoreDynFunction::Field | CoreDynFunction::Fields => {
            let fields = if shape.constructor == T::Dict {
                reference
                    .dict_fields()
                    .ok_or("Dyn dictionary has no fields")?
                    .into_iter()
                    .map(|name| (name, shape.arguments[0]))
                    .collect()
            } else {
                solved_dyn_fields(ty, types)?
            };
            if operation == CoreDynFunction::Field {
                let name = field.expect("field argument");
                let (_, ty) = fields
                    .iter()
                    .find(|(key, _)| *key == name)
                    .ok_or_else(|| format!("Dyn record has no field {name:?}"))?;
                let child = reference
                    .dict_get(name)
                    .ok_or("solved record field is missing")?
                    .value;
                Ok(DynObservation::Child(metadata(*ty), child))
            } else {
                Ok(DynObservation::NamedChildren(
                    fields
                        .into_iter()
                        .map(|(name, ty)| {
                            let child = reference
                                .dict_get(name)
                                .ok_or("solved record field is missing")?
                                .value;
                            Ok((name.to_owned(), metadata(ty), child))
                        })
                        .collect::<Result<_, String>>()?,
                ))
            }
        }
        CoreDynFunction::ArrayItems | CoreDynFunction::TupleItems => {
            let members = match (&shape.constructor, operation) {
                (T::Array, CoreDynFunction::ArrayItems) => vec![
                    shape.arguments[0];
                    reference.sequence_len().ok_or(
                        "Dyn Array has no elements"
                    )?
                ],
                (T::Tuple, CoreDynFunction::TupleItems) => shape.arguments.clone(),
                (T::Nominal(symbol), CoreDynFunction::TupleItems)
                    if types
                        .definition(*symbol)
                        .is_some_and(|d| d.operation == TypeOperation::Newtype) =>
                {
                    types
                        .layout(ty)
                        .ok_or("missing newtype layout")?
                        .members
                        .iter()
                        .map(|ty| ty.expect("newtype payload"))
                        .collect()
                }
                _ => return Err("Dyn sequence access has the wrong type".into()),
            };
            if reference.sequence_len() != Some(members.len()) {
                return Err("Dyn sequence differs from its solved layout".into());
            }
            Ok(DynObservation::Children(
                members
                    .into_iter()
                    .enumerate()
                    .map(|(index, ty)| (metadata(ty), reference.sequence_get(index).unwrap().value))
                    .collect(),
            ))
        }
        CoreDynFunction::Tag | CoreDynFunction::Payload => {
            let (_, tag, child) = solved_dyn_variant(ty, value, types, view)?;
            if operation == CoreDynFunction::Tag {
                Ok(DynObservation::Tag(tag))
            } else {
                Ok(DynObservation::Payload(
                    child.map(|(ty, value)| (metadata(ty), value)),
                ))
            }
        }
        _ => unreachable!("structural Dyn observation"),
    }
}
