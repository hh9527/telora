struct BootstrapPrelude {
    types: HashMap<String, TypeDescriptor>,
    schemes: HashMap<String, TypeScheme>,
}

impl BootstrapPrelude {
    fn new() -> Self {
        let artifact = Self {
            types: core_prelude_types(),
            schemes: core_prelude_schemes(),
        };
        debug_assert!(
            artifact
                .schemes
                .keys()
                .all(|name| artifact.types.contains_key(name))
        );
        artifact
    }
}

fn core_prelude_types() -> HashMap<String, TypeDescriptor> {
    let metadata = TypeDescriptor::Type;
    let function =
        |parameters: Vec<TypeDescriptor>, result: TypeDescriptor| TypeDescriptor::Function {
            parameters,
            result: Box::new(result),
        };
    let mut prelude = HashMap::new();
    for (name, instance) in [
        ("Type", TypeDescriptor::Type),
        ("Dyn", TypeDescriptor::Dyn),
        ("Any", TypeDescriptor::Any),
        ("Never", TypeDescriptor::Never),
        ("Int", TypeDescriptor::Int),
        ("Float", TypeDescriptor::Float),
        ("String", TypeDescriptor::String),
        ("Bytes", TypeDescriptor::Bytes),
        ("Atom", TypeDescriptor::AtomValue),
        ("Bool", normalized_bool_descriptor()),
        ("BlameError", blame_error_descriptor()),
    ] {
        prelude.insert(name.into(), TypeDescriptor::TypeOf(Box::new(instance)));
    }
    prelude.insert(
        "Array".into(),
        function(vec![metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "Dict".into(),
        function(vec![metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "TypeOf".into(),
        function(vec![metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "Tagged".into(),
        function(
            vec![TypeDescriptor::Any, metadata.clone()],
            metadata.clone(),
        ),
    );
    prelude.insert(
        "Tuple".into(),
        function(
            vec![TypeDescriptor::Array(Box::new(metadata.clone()))],
            metadata.clone(),
        ),
    );
    prelude.insert(
        "Func".into(),
        function(
            vec![
                TypeDescriptor::Array(Box::new(metadata.clone())),
                metadata.clone(),
            ],
            metadata.clone(),
        ),
    );
    for name in ["\0telora_struct", "\0telora_enum"] {
        prelude.insert(
            name.into(),
            function(
                vec![TypeDescriptor::Any, TypeDescriptor::Any],
                metadata.clone(),
            ),
        );
    }
    prelude.insert(
        "union".into(),
        function(
            vec![TypeDescriptor::Any, TypeDescriptor::Any],
            metadata.clone(),
        ),
    );
    prelude.insert(
        "Option".into(),
        function(vec![metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "Result".into(),
        function(vec![metadata.clone(), metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "FoldControl".into(),
        function(vec![metadata.clone(), metadata.clone()], metadata.clone()),
    );
    prelude.insert(
        "validate".into(),
        function(vec![metadata, TypeDescriptor::Any], TypeDescriptor::Any),
    );
    prelude.insert(
        "\0telora_warn".into(),
        function(
            vec![TypeDescriptor::String, TypeDescriptor::Any],
            TypeDescriptor::Atom(Atom::Builtin(BuiltinAtom::None)),
        ),
    );
    prelude.insert(
        "\0telora_pack_dyn".into(),
        function(
            vec![TypeDescriptor::Type, TypeDescriptor::Any],
            TypeDescriptor::Dyn,
        ),
    );
    prelude.insert(
        "\0telora_cast".into(),
        function(
            vec![TypeDescriptor::Type, TypeDescriptor::Any],
            TypeDescriptor::Any,
        ),
    );
    prelude
}

fn core_prelude_schemes() -> HashMap<String, TypeScheme> {
    let bound = |index| TypeDescriptor::Bound(TypeParameterId(index));
    let witness = |instance| TypeDescriptor::TypeOf(Box::new(instance));
    let function = |parameters, result| TypeDescriptor::Function {
        parameters,
        result: Box::new(result),
    };
    let scheme = |body| TypeScheme {
        parameters: Vec::new(),
        constraints: Vec::new(),
        body,
    };
    HashMap::from([
        (
            "Array".into(),
            scheme(function(
                vec![witness(bound(0))],
                witness(TypeDescriptor::Array(Box::new(bound(0)))),
            )),
        ),
        (
            "Dict".into(),
            scheme(function(
                vec![witness(bound(0))],
                witness(TypeDescriptor::Dict(Box::new(bound(0)))),
            )),
        ),
        (
            "TypeOf".into(),
            scheme(function(
                vec![witness(bound(0))],
                witness(witness(bound(0))),
            )),
        ),
        (
            "Option".into(),
            scheme(function(
                vec![witness(bound(0))],
                witness(option_descriptor(bound(0))),
            )),
        ),
        (
            "Result".into(),
            scheme(function(
                vec![witness(bound(0)), witness(bound(1))],
                witness(result_descriptor(bound(0), bound(1))),
            )),
        ),
        (
            "FoldControl".into(),
            scheme(function(
                vec![witness(bound(0)), witness(bound(1))],
                witness(fold_control_descriptor(bound(0), bound(1))),
            )),
        ),
        (
            "validate".into(),
            scheme(function(
                vec![witness(bound(0)), TypeDescriptor::Any],
                result_descriptor(bound(0), blame_error_descriptor()),
            )),
        ),
        (
            "\0telora_warn".into(),
            scheme(function(
                vec![TypeDescriptor::String, TypeDescriptor::Any],
                TypeDescriptor::Atom(Atom::Builtin(BuiltinAtom::None)),
            )),
        ),
    ])
}

pub(crate) fn audit_default_prelude_interface(interface: &ModuleInterface) -> Result<(), String> {
    let expected = ["PropertyAttr", "union", "validate"]
        .into_iter()
        .collect::<BTreeSet<_>>();
    let actual = interface
        .exports
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err("std/prelude must export exactly PropertyAttr, union, and validate".into());
    }
    let bootstrap = core_prelude_schemes();
    let expected_validate = &bootstrap["validate"];
    let declared_validate = &interface.exports["validate"];
    if declared_validate.body != expected_validate.body {
        return Err(format!(
            "std/prelude validate scheme {} differs from bootstrap {}",
            declared_validate.display_name(),
            expected_validate.display_name()
        ));
    }
    Ok(())
}

fn option_descriptor(item: TypeDescriptor) -> TypeDescriptor {
    TypeDescriptor::Enum(BTreeMap::from([
        ("None".into(), None),
        ("Some".into(), Some(Box::new(item))),
    ]))
}

fn option_parts(
    variants: &BTreeMap<String, Option<Box<TypeDescriptor>>>,
) -> Option<&TypeDescriptor> {
    (variants.len() == 2 && variants.get("None").is_some_and(Option::is_none))
        .then(|| variants.get("Some").and_then(Option::as_deref))
        .flatten()
}

fn blame_error_descriptor() -> TypeDescriptor {
    TypeDescriptor::Struct(BTreeMap::from([
        ("data".into(), TypeDescriptor::Any),
        ("message".into(), TypeDescriptor::String),
        ("rule".into(), TypeDescriptor::Any),
    ]))
}

fn result_descriptor(ok: TypeDescriptor, err: TypeDescriptor) -> TypeDescriptor {
    TypeDescriptor::Enum(BTreeMap::from([
        ("Err".into(), Some(Box::new(err))),
        ("Ok".into(), Some(Box::new(ok))),
    ]))
}

fn result_parts(descriptor: &TypeDescriptor) -> Option<(&TypeDescriptor, &TypeDescriptor)> {
    let TypeDescriptor::Enum(variants) = descriptor else {
        return None;
    };
    if variants.len() != 2 {
        return None;
    }
    Some((
        variants.get("Ok")?.as_deref()?,
        variants.get("Err")?.as_deref()?,
    ))
}

fn fold_control_descriptor(state: TypeDescriptor, result: TypeDescriptor) -> TypeDescriptor {
    TypeDescriptor::Enum(BTreeMap::from([
        ("Break".into(), Some(Box::new(result))),
        ("Continue".into(), Some(Box::new(state))),
    ]))
}

fn normalized_bool_descriptor() -> TypeDescriptor {
    TypeDescriptor::Enum(BTreeMap::from([
        ("False".into(), None),
        ("True".into(), None),
    ]))
}

fn native_array_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let item = context.argument(0)?;
    let value = context.value(item)?;
    if !value.is_hidden_type_slot() {
        validate_native_type(value)?;
    }
    write_native_type_record(context, "Array", &[("item", item)])
}

fn native_dict_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let item = context.argument(0)?;
    let value = context.value(item)?;
    if !value.is_hidden_type_slot() {
        validate_native_type(value)?;
    }
    write_native_type_record(context, "Dict", &[("item", item)])
}

fn native_type_of_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let instance = context.argument(0)?;
    let value = context.value(instance)?;
    if !value.is_hidden_type_slot() {
        validate_native_type(value)?;
    }
    write_native_type_record(context, "TypeOf", &[("instance", instance)])
}

pub(crate) fn native_value_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let value = context.argument(0)?;
    context.set_value_type(context.result(), value)
}

fn native_tagged_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let tag = context.argument(0)?;
    if context.value(tag)?.as_atom().is_none() {
        return Err(NativeError::new("Tagged expects an Atom tag"));
    }
    let payload = context.argument(1)?;
    let value = context.value(payload)?;
    if !value.is_hidden_type_slot() {
        validate_native_type(value)?;
    }
    write_native_type_record(context, "Tagged", &[("tag", tag), ("payload", payload)])
}

fn native_tuple_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let value = context.value(context.argument(0)?)?;
    if value.kind() != ValueKind::Array {
        return Err(NativeError::new("Tuple expects an Array of Types"));
    }
    for index in 0..value.sequence_len().expect("Array has a length") {
        let item = value.sequence_get(index).expect("valid Array index");
        if !item.is_hidden_type_slot() {
            validate_native_type(item)?;
        }
    }
    write_native_type_record(context, "Tuple", &[("items", context.argument(0)?)])
}

fn native_function_type(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let parameters_value = context.value(context.argument(0)?)?;
    if parameters_value.kind() != ValueKind::Array {
        return Err(NativeError::new("Func expects an Array of parameter Types"));
    }
    for index in 0..parameters_value.sequence_len().expect("Array has a length") {
        let parameter = parameters_value
            .sequence_get(index)
            .expect("valid Array index");
        if !parameter.is_hidden_type_slot() {
            validate_native_type(parameter)?;
        }
    }
    let result = context.argument(1)?;
    let result_value = context.value(result)?;
    if !result_value.is_hidden_type_slot() {
        validate_native_type(result_value)?;
    }
    write_native_type_record(
        context,
        "Func",
        &[("parameters", context.argument(0)?), ("result", result)],
    )
}

fn write_native_type_record(
    context: &mut CallContext<'_, '_>,
    kind_name: &str,
    preserved_fields: &[(&str, RegisterId)],
) -> Result<(), NativeError> {
    let kind = context.scratch()?;
    context.set_atom(kind, kind_name)?;
    let mut fields = Vec::with_capacity(preserved_fields.len() + 1);
    fields.push(("kind".to_owned(), kind));
    fields.extend(
        preserved_fields
            .iter()
            .map(|(name, register)| ((*name).to_owned(), *register)),
    );
    context.make_dict(context.result(), &fields)
}

pub(crate) fn native_validate(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let type_register = context.argument(0)?;
    let value_register = context.argument(1)?;
    let descriptor = decode_native_type(context.value(type_register)?)?;
    let tag = context.scratch()?;
    let payload = context.scratch()?;
    match validate_value_ref(&descriptor, context.value(value_register)?, "value") {
        Ok(()) => {
            context.set_atom(tag, "Ok")?;
            if matches!(descriptor, TypeDescriptor::Declared(_))
                && context
                    .value(value_register)?
                    .declared_value_parts()
                    .is_none()
            {
                context.make_declared_value(payload, type_register, value_register)?;
            } else {
                context.copy(payload, value_register)?;
            }
        }
        Err(message) => {
            context.set_atom(tag, "Err")?;
            let error_message = context.scratch()?;
            context.set_string(error_message, message)?;
            context.make_dict(
                payload,
                &[
                    ("message".into(), error_message),
                    ("data".into(), value_register),
                    ("rule".into(), type_register),
                ],
            )?;
        }
    }
    context.make_tagged(context.result(), tag, payload)
}

fn native_checked_cast(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let type_register = context.argument(0)?;
    let value_register = context.argument(1)?;
    let descriptor = decode_native_type(context.value(type_register)?)?;
    let tag = context.scratch()?;
    let payload = context.scratch()?;
    match validate_value_ref(&descriptor, context.value(value_register)?, "value") {
        Ok(()) => {
            context.set_atom(tag, "Ok")?;
            if matches!(descriptor, TypeDescriptor::Declared(_))
                && context
                    .value(value_register)?
                    .declared_value_parts()
                    .is_none()
            {
                context.make_declared_value(payload, type_register, value_register)?;
            } else {
                context.copy(payload, value_register)?;
            }
        }
        Err(message) => {
            context.set_atom(tag, "Err")?;
            context.set_string(payload, message)?;
        }
    }
    context.make_tagged(context.result(), tag, payload)
}

pub(crate) fn decode_native_type(value: ValueRef<'_>) -> Result<TypeDescriptor, NativeError> {
    decode_type_ref_with(value, "Type", false).map_err(NativeError::new)
}

fn validate_native_type(value: ValueRef<'_>) -> Result<(), NativeError> {
    TypeGraph::default()
        .decode_persistent(value, "Type", &mut HashMap::new())
        .map(|_| ())
        .map_err(NativeError::new)
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
            TypeDescriptor::Any
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
            require(&["kind"])?;
            TypeDescriptor::Any
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
        "Atom" => {
            if fields.iter().copied().eq(["kind"]) {
                return Ok(TypeDescriptor::AtomValue);
            }
            require(&["kind", "tag"])?;
            let tag = value
                .dict_get("tag")
                .and_then(ValueRef::as_atom)
                .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
            TypeDescriptor::Atom(atom_from_name(tag.as_str()))
        }
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
        "Tagged" => {
            require(&["kind", "payload", "tag"])?;
            let tag = value
                .dict_get("tag")
                .and_then(ValueRef::as_atom)
                .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
            let payload = value
                .dict_get("payload")
                .ok_or_else(|| format!("{path}.payload is missing"))?;
            TypeDescriptor::Tagged {
                tag: atom_from_name(tag.as_str()),
                payload: Box::new(decode_type_ref_with_visiting(
                    payload,
                    &format!("{path}.payload"),
                    shallow_declared_types,
                    visiting_declared,
                )?),
            }
        }
        "Tuple" | "Union" => {
            let field = if kind == "Tuple" { "items" } else { "variants" };
            if kind == "Tuple" {
                require(&["items", "kind"])?;
            } else {
                require(&["kind", "variants"])?;
            }
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
            if kind == "Union" && values.is_empty() {
                return Err(format!("{path}.variants must not be empty"));
            }
            if kind == "Tuple" {
                TypeDescriptor::Tuple(values)
            } else {
                TypeDescriptor::Union(values)
            }
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

fn validate_value_ref(
    descriptor: &TypeDescriptor,
    value: ValueRef<'_>,
    path: &str,
) -> Result<(), String> {
    match descriptor {
        TypeDescriptor::Declared(expected) => {
            if let Some((owner, _)) = value.declared_value_parts() {
                let Some((actual, _, _)) = owner.declared_type_parts() else {
                    return Err(format!("{path} has an invalid declared owner"));
                };
                if actual != &expected.id {
                    return Err(format!("{path} has a different declared type identity"));
                }
                Ok(())
            } else {
                validate_value_ref(&expected.body, value, path)
            }
        }
        TypeDescriptor::Any => Ok(()),
        TypeDescriptor::Never => Err(format!("{path} cannot have type Never")),
        TypeDescriptor::Type => decode_type_ref(value, path).map(|_| ()),
        TypeDescriptor::Dyn if value.kind() == ValueKind::Dyn => Ok(()),
        TypeDescriptor::TypeOf(expected) => {
            let actual = decode_type_ref(value, path)?;
            if assignable(&actual, expected) && assignable(expected, &actual) {
                Ok(())
            } else {
                Err(format!(
                    "{path} must describe {}, got {}",
                    expected.display_name(),
                    actual.display_name()
                ))
            }
        }
        TypeDescriptor::Int if value.kind() == ValueKind::Int => Ok(()),
        TypeDescriptor::Float if value.kind() == ValueKind::Float => Ok(()),
        TypeDescriptor::String if value.kind() == ValueKind::String => Ok(()),
        TypeDescriptor::Bytes if value.kind() == ValueKind::Bytes => Ok(()),
        TypeDescriptor::AtomValue if value.kind() == ValueKind::Atom => Ok(()),
        TypeDescriptor::Opaque(expected) if value.kind() == ValueKind::Opaque => {
            let actual = value.opaque_native_type().expect("ValueKind checked");
            if actual == expected {
                Ok(())
            } else {
                Err(format!(
                    "{path} must be {}, got {}",
                    expected.qualified_name(),
                    actual.qualified_name()
                ))
            }
        }
        TypeDescriptor::Atom(expected)
            if value.as_atom().is_some_and(|atom| atom == expected.name()) =>
        {
            Ok(())
        }
        TypeDescriptor::Atom(expected) => Err(format!("{path} must be '{}", expected.name())),
        TypeDescriptor::Array(item) => {
            if value.kind() != ValueKind::Array {
                return Err(format!("{path} must be an Array"));
            }
            for index in 0..value.sequence_len().expect("Array has a length") {
                validate_value_ref(
                    item,
                    value.sequence_get(index).expect("valid Array index"),
                    &format!("{path}[{index}]"),
                )?;
            }
            Ok(())
        }
        TypeDescriptor::Dict(item) => {
            let Some(names) = value.dict_fields() else {
                return Err(format!("{path} must be a Dict"));
            };
            for name in names {
                validate_value_ref(
                    item,
                    value.dict_get(name).expect("Dict field exists"),
                    &format!("{path}.{name}"),
                )?;
            }
            Ok(())
        }
        TypeDescriptor::Tagged { tag, payload } => {
            let Some((actual_tag, actual_payload)) = value.tagged_parts() else {
                return Err(format!("{path} must be a Tagged value"));
            };
            if !actual_tag
                .as_atom()
                .is_some_and(|actual| actual == tag.name())
            {
                return Err(format!("{path} must have tag '{}", tag.name()));
            }
            validate_value_ref(payload, actual_payload, &format!("{path}.payload"))
        }
        TypeDescriptor::Tuple(items) => {
            if value.kind() != ValueKind::Tuple {
                return Err(format!("{path} must be a Tuple"));
            }
            if value.sequence_len() != Some(items.len()) {
                return Err(format!("{path} must have {} tuple items", items.len()));
            }
            for (index, item) in items.iter().enumerate() {
                validate_value_ref(
                    item,
                    value.sequence_get(index).expect("valid Tuple index"),
                    &format!("{path}.{index}"),
                )?;
            }
            Ok(())
        }
        TypeDescriptor::Struct(items) => {
            let Some(names) = value.dict_fields() else {
                return Err(format!("{path} must be a Dict"));
            };
            if !items.keys().eq(names.iter()) {
                return Err(format!("{path} has a different field shape"));
            }
            for (name, item) in items {
                validate_value_ref(
                    item,
                    value.dict_get(name).expect("matching shape"),
                    &format!("{path}.{name}"),
                )?;
            }
            Ok(())
        }
        TypeDescriptor::Enum(variants) => {
            if let Some(tag) = value.as_atom() {
                return match variants.get(tag.as_str()) {
                    Some(None) => Ok(()),
                    Some(Some(_)) => Err(format!("{path} variant '{tag} requires a payload")),
                    None => Err(format!("{path} has unknown Enum variant '{tag}")),
                };
            }
            let Some((tag_value, payload_value)) = value.tagged_parts() else {
                return Err(format!("{path} must be a unit Atom or a Tagged value"));
            };
            let tag = tag_value
                .as_atom()
                .ok_or_else(|| format!("{path} Tagged tag must be an Atom"))?;
            match variants.get(tag.as_str()) {
                Some(Some(payload)) => {
                    validate_value_ref(payload, payload_value, &format!("{path}.{tag}"))
                }
                Some(None) => Err(format!("{path} variant '{tag} does not accept a payload")),
                None => Err(format!("{path} has unknown Enum variant '{tag}")),
            }
        }
        TypeDescriptor::Union(variants) => {
            if variants
                .iter()
                .any(|variant| validate_value_ref(variant, value, path).is_ok())
            {
                Ok(())
            } else {
                Err(format!("{path} does not match any Union variant"))
            }
        }
        TypeDescriptor::Function { parameters, .. }
            if value.function_arity() == Some(parameters.len()) =>
        {
            Ok(())
        }
        TypeDescriptor::Function { parameters, .. } if value.kind() == ValueKind::Func => {
            Err(format!("{path} must accept {} arguments", parameters.len()))
        }
        descriptor => Err(format!(
            "{path} must be {}, got {:?}",
            descriptor.display_name(),
            value.kind()
        )),
    }
}

fn infer_expr(expression: &Expr, environment: &HashMap<String, TypeDescriptor>) -> TypeDescriptor {
    infer_expr_with(expression, environment, &mut |_, _| {})
}

fn collect_declared_bodies(
    descriptor: &TypeDescriptor,
    bodies: &mut HashMap<crate::value::DeclaredTypeId, Arc<TypeDescriptor>>,
    visiting: &mut HashSet<crate::value::DeclaredTypeId>,
) {
    let visit = |descriptor: &TypeDescriptor,
                 bodies: &mut HashMap<crate::value::DeclaredTypeId, Arc<TypeDescriptor>>,
                 visiting: &mut HashSet<crate::value::DeclaredTypeId>| {
        collect_declared_bodies(descriptor, bodies, visiting)
    };
    match descriptor {
        TypeDescriptor::Declared(declared) => {
            if matches!(
                declared.body.as_ref(),
                TypeDescriptor::Struct(_) | TypeDescriptor::Enum(_)
            ) {
                bodies
                    .entry(declared.id.clone())
                    .or_insert_with(|| Arc::clone(&declared.body));
            }
            if visiting.insert(declared.id.clone()) {
                visit(&declared.body, bodies, visiting);
                visiting.remove(&declared.id);
            }
            for argument in declared.id.arguments() {
                visit(argument, bodies, visiting);
            }
        }
        TypeDescriptor::TypeOf(inner)
        | TypeDescriptor::Array(inner)
        | TypeDescriptor::Dict(inner) => visit(inner, bodies, visiting),
        TypeDescriptor::Tagged { payload, .. } => visit(payload, bodies, visiting),
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            for item in items {
                visit(item, bodies, visiting);
            }
        }
        TypeDescriptor::Struct(fields) => {
            for field in fields.values() {
                visit(field, bodies, visiting);
            }
        }
        TypeDescriptor::Enum(variants) => {
            for payload in variants.values().flatten() {
                visit(payload, bodies, visiting);
            }
        }
        TypeDescriptor::Function { parameters, result } => {
            for parameter in parameters {
                visit(parameter, bodies, visiting);
            }
            visit(result, bodies, visiting);
        }
        TypeDescriptor::Bound(_)
        | TypeDescriptor::Named(_)
        | TypeDescriptor::Inference(_)
        | TypeDescriptor::Any
        | TypeDescriptor::Never
        | TypeDescriptor::Type
        | TypeDescriptor::Dyn
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::AtomValue
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Atom(_) => {}
    }
}
