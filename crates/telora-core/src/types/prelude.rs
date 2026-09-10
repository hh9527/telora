pub(crate) fn unchecked_type_constructor() -> crate::TypeConstructorId {
    crate::TypeConstructorId { module: crate::ModuleId::ANONYMOUS, local: 2 }
}

pub(crate) fn unchecked_descriptor(target: TypeDescriptor) -> TypeDescriptor {
    if matches!(&target, TypeDescriptor::Declared(declared)
        if declared.id.constructor() == unchecked_type_constructor()) {
        return target;
    }
    let body = match &target {
        TypeDescriptor::Declared(declared) => Arc::clone(&declared.body),
        _ => Arc::new(TypeDescriptor::Struct(BTreeMap::new())),
    };
    let constructor = unchecked_type_constructor();
    TypeDescriptor::Declared(DeclaredTypeDescriptor {
        id: crate::value::DeclaredTypeId::applied(constructor.module, constructor.local, &[target]),
        name: "Unchecked".into(),
        body,
    })
}

pub(crate) fn decode_type_ref(value: ValueRef<'_>, path: &str) -> Result<TypeDescriptor, String> {
    let mut graph = TypeGraph::default();
    let root = graph.decode_persistent(value, path, &mut HashMap::new())?;
    graph.descriptor(root)
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

fn decode_type_ref_with(
    value: ValueRef<'_>,
    path: &str,
    shallow_declared_types: bool,
) -> Result<TypeDescriptor, String> {
    decode_type_ref_with_visiting(value, path, shallow_declared_types, &mut HashSet::new())
}

fn decode_type_ref_with_visiting(
    value: ValueRef<'_>,
    path: &str,
    shallow_declared_types: bool,
    visiting_declared: &mut HashSet<crate::value::DeclaredTypeId>,
) -> Result<TypeDescriptor, String> {
    let value = value.resolve_hidden_type_slot()?;
    if let Some(native_type) = value.as_native_type() {
        return Ok(TypeDescriptor::Opaque(native_type.clone()));
    }
    if let Some((id, name, body)) = value.declared_type_parts() {
        let recursive = !visiting_declared.insert(id.clone());
        if recursive {
            return Ok(TypeDescriptor::Named(name.to_owned()));
        }
        let decoded_body = if shallow_declared_types {
            TypeDescriptor::Named(name.to_owned())
        } else {
            let body = decode_type_ref_with_visiting(body, path, false, visiting_declared)?;
            visiting_declared.remove(id);
            body
        };
        return Ok(TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: id.clone(),
            name: name.to_owned(),
            body: Arc::new(decoded_body),
        }));
    }
    let fields = value
        .dict_fields()
        .ok_or_else(|| format!("{path} must be a Dict"))?;
    let kind = value
        .dict_get("kind")
        .and_then(ValueRef::as_atom)
        .ok_or_else(|| format!("{path}.kind must be an Atom"))?;
    if kind.as_str() == "Union" {
        return Err(format!("{path}: Union has been removed; use an explicit enum"));
    }
    let require = |expected: &[&str]| -> Result<(), String> {
        if fields.iter().copied().eq(expected.iter().copied()) {
            Ok(())
        } else {
            Err(format!("{path} has invalid fields for {kind}"))
        }
    };
    Ok(match kind.as_str() {
        "Bound" | "'Bound" => {
            require(&["kind", "parameter"])?;
            let parameter = value
                .dict_get("parameter")
                .and_then(ValueRef::as_int)
                .and_then(|parameter| u32::try_from(parameter).ok())
                .ok_or_else(|| format!("{path}.parameter must be a non-negative Int"))?;
            TypeDescriptor::Bound(TypeParameterId(parameter))
        }
        "Named" => {
            require(&["kind", "name"])?;
            let name = value
                .dict_get("name")
                .and_then(ValueRef::as_str)
                .ok_or_else(|| format!("{path}.name must be a String"))?;
            TypeDescriptor::Named(name.as_str().to_owned())
        }
        "Any" => {
            return Err(format!("{path}: Any is not a supported type"));
        }
        "Never" => {
            require(&["kind"])?;
            TypeDescriptor::Never
        }
        "Type" => {
            require(&["kind"])?;
            TypeDescriptor::Type
        }
        "Dyn" => {
            require(&["kind"])?;
            TypeDescriptor::Dyn
        }
        "TypeOf" => {
            require(&["instance", "kind"])?;
            let instance = value
                .dict_get("instance")
                .ok_or_else(|| format!("{path}.instance is missing"))?;
            TypeDescriptor::TypeOf(Box::new(decode_type_ref_with_visiting(
                instance,
                &format!("{path}.instance"),
                shallow_declared_types,
                visiting_declared,
            )?))
        }
        "Int" => {
            require(&["kind"])?;
            TypeDescriptor::Int
        }
        "Float" => {
            require(&["kind"])?;
            TypeDescriptor::Float
        }
        "String" => {
            require(&["kind"])?;
            TypeDescriptor::String
        }
        "Bytes" => {
            require(&["kind"])?;
            TypeDescriptor::Bytes
        }
        "Atom" | "Tagged" => return Err(format!("{path}: standalone {kind} is not a supported type; use an enum")),
        "Array" => {
            require(&["item", "kind"])?;
            let item = value
                .dict_get("item")
                .ok_or_else(|| format!("{path}.item is missing"))?;
            TypeDescriptor::Array(Box::new(decode_type_ref_with_visiting(
                item,
                &format!("{path}.item"),
                shallow_declared_types,
                visiting_declared,
            )?))
        }
        "Dict" => {
            require(&["item", "kind"])?;
            let item = value
                .dict_get("item")
                .ok_or_else(|| format!("{path}.item is missing"))?;
            TypeDescriptor::Dict(Box::new(decode_type_ref_with_visiting(
                item,
                &format!("{path}.item"),
                shallow_declared_types,
                visiting_declared,
            )?))
        }
        "Tuple" => {
            let field = "items";
            require(&["items", "kind"])?;
            let sequence = value
                .dict_get(field)
                .ok_or_else(|| format!("{path}.{field} is missing"))?;
            if sequence.kind() != ValueKind::Array {
                return Err(format!("{path}.{field} must be an Array"));
            }
            let values = (0..sequence.sequence_len().expect("Array has a length"))
                .map(|index| {
                    decode_type_ref_with_visiting(
                        sequence.sequence_get(index).expect("valid Array index"),
                        &format!("{path}.{field}[{index}]"),
                        shallow_declared_types,
                        visiting_declared,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            TypeDescriptor::Tuple(values)
        }
        "Newtype" => {
            require(&["kind", "payload"])?;
            let payload = value.dict_get("payload")
                .ok_or_else(|| format!("{path}.payload is missing"))?;
            TypeDescriptor::Newtype(Box::new(decode_type_ref_with_visiting(
                payload,
                &format!("{path}.payload"),
                shallow_declared_types,
                visiting_declared,
            )?))
        }
        "Struct" => {
            require(&["fields", "kind"])?;
            let fields_value = value
                .dict_get("fields")
                .ok_or_else(|| format!("{path}.fields is missing"))?;
            let names = fields_value
                .dict_fields()
                .ok_or_else(|| format!("{path}.fields must be a Dict"))?;
            TypeDescriptor::Struct(
                names
                    .iter()
                    .map(|name| {
                        let field = fields_value.dict_get(name).expect("Dict field exists");
                        Ok((
                            (*name).to_owned(),
                            decode_type_ref_with_visiting(
                                field,
                                &format!("{path}.fields.{name}"),
                                shallow_declared_types,
                                visiting_declared,
                            )?,
                        ))
                    })
                    .collect::<Result<_, String>>()?,
            )
        }
        "Enum" => {
            require(&["kind", "variants"])?;
            let variants = value
                .dict_get("variants")
                .ok_or_else(|| format!("{path}.variants is missing"))?;
            let names = variants
                .dict_fields()
                .ok_or_else(|| format!("{path}.variants must be a Dict"))?;
            if names.is_empty() {
                return Err(format!("{path}.variants must not be empty"));
            }
            TypeDescriptor::Enum(
                names
                    .iter()
                    .map(|name| {
                        let variant = variants.dict_get(name).expect("Dict field exists");
                        let variant_path = format!("{path}.variants.{name}");
                        let payload = if variant.as_atom().is_some_and(|atom| atom == "None") {
                            None
                        } else {
                            Some(Box::new(decode_type_ref_with_visiting(
                                variant,
                                &variant_path,
                                shallow_declared_types,
                                visiting_declared,
                            )?))
                        };
                        Ok(((*name).to_owned(), payload))
                    })
                    .collect::<Result<_, String>>()?,
            )
        }
        "Func" => {
            require(&["kind", "parameters", "result"])?;
            let parameters = value
                .dict_get("parameters")
                .ok_or_else(|| format!("{path}.parameters is missing"))?;
            if parameters.kind() != ValueKind::Array {
                return Err(format!("{path}.parameters must be an Array"));
            }
            let parameters = (0..parameters.sequence_len().expect("Array has a length"))
                .map(|index| {
                    decode_type_ref_with_visiting(
                        parameters.sequence_get(index).expect("valid Array index"),
                        &format!("{path}.parameters[{index}]"),
                        shallow_declared_types,
                        visiting_declared,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            let result = value
                .dict_get("result")
                .ok_or_else(|| format!("{path}.result is missing"))?;
            TypeDescriptor::Function {
                parameters,
                result: Box::new(decode_type_ref_with_visiting(
                    result,
                    &format!("{path}.result"),
                    shallow_declared_types,
                    visiting_declared,
                )?),
            }
        }
        _ => return Err(format!("{path}.kind has unknown value '{kind}")),
    })
}
