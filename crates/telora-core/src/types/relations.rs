pub(crate) fn apply_declared_type_arguments(
    id: &crate::value::DeclaredTypeId,
    arguments: &[TypeDescriptor],
) -> crate::value::DeclaredTypeId {
    let replacements = arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| {
            (
                TypeParameterId(u32::try_from(index).expect("type family arity exceeds u32")),
                argument.clone(),
            )
        })
        .collect::<HashMap<_, _>>();
    let applied = id
        .arguments()
        .iter()
        .map(|argument| substitute_bound_parameters(argument, &replacements))
        .collect::<Vec<_>>();
    id.reapply(&applied)
}

pub(crate) fn type_identity_contains_bound_parameter(descriptor: &TypeDescriptor) -> bool {
    match descriptor {
        TypeDescriptor::Bound(_) => true,
        TypeDescriptor::Declared(declared) => declared
            .id
            .arguments()
            .iter()
            .any(type_identity_contains_bound_parameter),
        TypeDescriptor::Array(item) | TypeDescriptor::Dict(item) | TypeDescriptor::TypeOf(item) => {
            type_identity_contains_bound_parameter(item)
        }
        TypeDescriptor::Tagged { payload, .. } => type_identity_contains_bound_parameter(payload),
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            items.iter().any(type_identity_contains_bound_parameter)
        }
        TypeDescriptor::Struct(fields) => {
            fields.values().any(type_identity_contains_bound_parameter)
        }
        TypeDescriptor::Enum(variants) => variants.values().any(|payload| {
            payload
                .as_deref()
                .is_some_and(type_identity_contains_bound_parameter)
        }),
        TypeDescriptor::Function { parameters, result } => {
            parameters
                .iter()
                .any(type_identity_contains_bound_parameter)
                || type_identity_contains_bound_parameter(result)
        }
        TypeDescriptor::Named(_)
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
        | TypeDescriptor::Atom(_) => false,
    }
}

pub(crate) fn type_identity_is_symbolic(descriptor: &TypeDescriptor) -> bool {
    match descriptor {
        TypeDescriptor::Bound(_) | TypeDescriptor::Named(_) | TypeDescriptor::Inference(_) => true,
        TypeDescriptor::Declared(declared) => declared
            .id
            .arguments()
            .iter()
            .any(type_identity_is_symbolic),
        TypeDescriptor::Array(item) | TypeDescriptor::Dict(item) | TypeDescriptor::TypeOf(item) => {
            type_identity_is_symbolic(item)
        }
        TypeDescriptor::Tagged { payload, .. } => type_identity_is_symbolic(payload),
        TypeDescriptor::Tuple(items) | TypeDescriptor::Union(items) => {
            items.iter().any(type_identity_is_symbolic)
        }
        TypeDescriptor::Struct(fields) => fields.values().any(type_identity_is_symbolic),
        TypeDescriptor::Enum(variants) => variants
            .values()
            .any(|payload| payload.as_deref().is_some_and(type_identity_is_symbolic)),
        TypeDescriptor::Function { parameters, result } => {
            parameters.iter().any(type_identity_is_symbolic) || type_identity_is_symbolic(result)
        }
        TypeDescriptor::Any
        | TypeDescriptor::Never
        | TypeDescriptor::Type
        | TypeDescriptor::Dyn
        | TypeDescriptor::Int
        | TypeDescriptor::Float
        | TypeDescriptor::String
        | TypeDescriptor::Bytes
        | TypeDescriptor::AtomValue
        | TypeDescriptor::Opaque(_)
        | TypeDescriptor::Atom(_) => false,
    }
}

pub(crate) fn erase_type_variables(descriptor: &TypeDescriptor) -> TypeDescriptor {
    match descriptor {
        TypeDescriptor::Bound(_) | TypeDescriptor::Inference(_) => TypeDescriptor::Any,
        TypeDescriptor::Declared(declared) => {
            let arguments = declared
                .id
                .arguments()
                .iter()
                .map(erase_type_variables)
                .collect::<Vec<_>>();
            TypeDescriptor::Declared(DeclaredTypeDescriptor {
                id: declared.id.reapply(&arguments),
                name: declared.name.clone(),
                body: Arc::new(erase_type_variables(&declared.body)),
            })
        }
        TypeDescriptor::Array(item) => TypeDescriptor::Array(Box::new(erase_type_variables(item))),
        TypeDescriptor::Dict(item) => TypeDescriptor::Dict(Box::new(erase_type_variables(item))),
        TypeDescriptor::TypeOf(instance) => {
            TypeDescriptor::TypeOf(Box::new(erase_type_variables(instance)))
        }
        TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
            tag: tag.clone(),
            payload: Box::new(erase_type_variables(payload)),
        },
        TypeDescriptor::Tuple(items) => {
            TypeDescriptor::Tuple(items.iter().map(erase_type_variables).collect())
        }
        TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), erase_type_variables(field)))
                .collect(),
        ),
        TypeDescriptor::Enum(variants) => TypeDescriptor::Enum(
            variants
                .iter()
                .map(|(name, payload)| {
                    (
                        name.clone(),
                        payload
                            .as_ref()
                            .map(|payload| Box::new(erase_type_variables(payload))),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Union(variants) => {
            TypeDescriptor::Union(variants.iter().map(erase_type_variables).collect())
        }
        TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
            parameters: parameters.iter().map(erase_type_variables).collect(),
            result: Box::new(erase_type_variables(result)),
        },
        descriptor => descriptor.clone(),
    }
}

fn join_all_types(types: Vec<TypeDescriptor>) -> TypeDescriptor {
    types.into_iter().fold(TypeDescriptor::Never, join_types)
}

fn potentially_assignable(actual: &TypeDescriptor, expected: &TypeDescriptor) -> bool {
    if matches!(actual, TypeDescriptor::Inference(_) | TypeDescriptor::Any)
        || matches!(expected, TypeDescriptor::Inference(_) | TypeDescriptor::Any)
    {
        return true;
    }
    match (actual, expected) {
        (TypeDescriptor::Array(actual), TypeDescriptor::Array(expected))
        | (TypeDescriptor::Dict(actual), TypeDescriptor::Dict(expected))
        | (TypeDescriptor::TypeOf(actual), TypeDescriptor::TypeOf(expected)) => {
            potentially_assignable(actual, expected)
        }
        (TypeDescriptor::Tuple(actual), TypeDescriptor::Tuple(expected))
            if actual.len() == expected.len() =>
        {
            actual
                .iter()
                .zip(expected)
                .all(|(actual, expected)| potentially_assignable(actual, expected))
        }
        (TypeDescriptor::Struct(actual), TypeDescriptor::Struct(expected))
            if actual.keys().eq(expected.keys()) =>
        {
            actual
                .iter()
                .all(|(name, actual)| potentially_assignable(actual, &expected[name]))
        }
        _ => assignable(actual, expected),
    }
}

fn expression_constructs_declared_value(expression: &Expr) -> bool {
    matches!(expression.value, ExprKind::Dict(_) | ExprKind::Atom(_))
        || matches!(
            &expression.value,
            ExprKind::Call { callee, .. } if matches!(callee.value, ExprKind::Atom(_))
        )
}

fn join_types(left: TypeDescriptor, right: TypeDescriptor) -> TypeDescriptor {
    if left == right {
        return left;
    }
    if matches!(left, TypeDescriptor::Never) {
        return right;
    }
    if matches!(right, TypeDescriptor::Never) {
        return left;
    }
    if matches!(left, TypeDescriptor::Any) || matches!(right, TypeDescriptor::Any) {
        return TypeDescriptor::Any;
    }
    if assignable(&left, &TypeDescriptor::Type) && assignable(&right, &TypeDescriptor::Type) {
        return TypeDescriptor::Type;
    }
    let left_to_right = assignable(&left, &right);
    let right_to_left = assignable(&right, &left);
    match (left_to_right, right_to_left) {
        (true, false) => right,
        (false, true) => left,
        _ => canonical_union(vec![left, right]),
    }
}

fn canonical_union(types: Vec<TypeDescriptor>) -> TypeDescriptor {
    fn flatten(ty: TypeDescriptor, flattened: &mut Vec<TypeDescriptor>) {
        match ty {
            TypeDescriptor::Union(variants) => {
                for variant in variants {
                    flatten(variant, flattened);
                }
            }
            ty => flattened.push(ty),
        }
    }

    let mut flattened = Vec::new();
    for ty in types {
        flatten(ty, &mut flattened);
    }
    if flattened.iter().any(|ty| matches!(ty, TypeDescriptor::Any)) {
        return TypeDescriptor::Any;
    }
    if flattened
        .iter()
        .any(|ty| !matches!(ty, TypeDescriptor::Never))
    {
        flattened.retain(|ty| !matches!(ty, TypeDescriptor::Never));
    }
    flattened.sort_by_cached_key(|ty| (ty.display_name(), format!("{ty:?}")));
    flattened.dedup();
    match flattened.len() {
        0 => TypeDescriptor::Never,
        1 => flattened.pop().expect("one canonical Union member"),
        _ => TypeDescriptor::Union(flattened),
    }
}

pub(crate) fn assignable(actual: &TypeDescriptor, expected: &TypeDescriptor) -> bool {
    match (actual, expected) {
        (TypeDescriptor::Never, _) => true,
        (TypeDescriptor::Any, _) | (_, TypeDescriptor::Any) => true,
        (TypeDescriptor::TypeOf(_), TypeDescriptor::Type) => true,
        (TypeDescriptor::TypeOf(actual), TypeDescriptor::TypeOf(expected)) => {
            assignable(actual, expected)
        }
        (TypeDescriptor::Atom(_), TypeDescriptor::AtomValue) => true,
        (TypeDescriptor::Declared(actual), TypeDescriptor::Declared(expected)) => {
            actual.id == expected.id
        }
        (TypeDescriptor::Enum(actual), TypeDescriptor::Enum(expected)) => {
            actual.len() == expected.len()
                && expected.iter().all(|(name, expected)| {
                    actual
                        .get(name)
                        .is_some_and(|actual| match (actual, expected) {
                            (None, None) => true,
                            (Some(actual), Some(expected)) => assignable(actual, expected),
                            _ => false,
                        })
                })
        }
        (TypeDescriptor::Union(variants), expected @ TypeDescriptor::Enum(_)) => {
            variants.iter().all(|variant| assignable(variant, expected))
        }
        (actual, TypeDescriptor::Enum(variants)) => variants.iter().any(|(name, payload)| {
            assignable(actual, &enum_variant_type(name, payload.as_deref()))
        }),
        (TypeDescriptor::Enum(variants), expected) => variants.iter().all(|(name, payload)| {
            assignable(&enum_variant_type(name, payload.as_deref()), expected)
        }),
        (TypeDescriptor::Union(actual), TypeDescriptor::Union(expected)) => actual
            .iter()
            .all(|actual| expected.iter().any(|expected| assignable(actual, expected))),
        (actual, TypeDescriptor::Union(variants)) => {
            variants.iter().any(|variant| assignable(actual, variant))
        }
        (TypeDescriptor::Union(variants), expected) => {
            variants.iter().all(|variant| assignable(variant, expected))
        }
        (TypeDescriptor::Array(actual), TypeDescriptor::Array(expected)) => {
            assignable(actual, expected)
        }
        (TypeDescriptor::Dict(actual), TypeDescriptor::Dict(expected)) => {
            assignable(actual, expected)
        }
        (TypeDescriptor::Struct(actual), TypeDescriptor::Dict(expected)) => {
            actual.values().all(|actual| assignable(actual, expected))
        }
        (
            TypeDescriptor::Tagged {
                tag: actual_tag,
                payload: actual,
            },
            TypeDescriptor::Tagged {
                tag: expected_tag,
                payload: expected,
            },
        ) => actual_tag == expected_tag && assignable(actual, expected),
        (TypeDescriptor::Tuple(actual), TypeDescriptor::Tuple(expected)) => {
            actual.len() == expected.len()
                && actual
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| assignable(actual, expected))
        }
        (TypeDescriptor::Struct(actual), TypeDescriptor::Struct(expected)) => {
            actual.len() == expected.len()
                && expected.iter().all(|(name, expected)| {
                    actual
                        .get(name)
                        .is_some_and(|actual| assignable(actual, expected))
                })
        }
        (
            TypeDescriptor::Function {
                parameters: actual_parameters,
                result: actual_result,
            },
            TypeDescriptor::Function {
                parameters: expected_parameters,
                result: expected_result,
            },
        ) => {
            actual_parameters.len() == expected_parameters.len()
                && actual_parameters
                    .iter()
                    .zip(expected_parameters)
                    .all(|(actual, expected)| assignable(actual, expected))
                && assignable(actual_result, expected_result)
        }
        _ => actual == expected,
    }
}

pub(crate) fn erase_declared_identity(descriptor: &TypeDescriptor) -> TypeDescriptor {
    match descriptor {
        TypeDescriptor::Declared(declared) => erase_declared_identity(&declared.body),
        TypeDescriptor::Array(item) => {
            TypeDescriptor::Array(Box::new(erase_declared_identity(item)))
        }
        TypeDescriptor::Dict(item) => TypeDescriptor::Dict(Box::new(erase_declared_identity(item))),
        TypeDescriptor::TypeOf(instance) => {
            TypeDescriptor::TypeOf(Box::new(erase_declared_identity(instance)))
        }
        TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
            tag: tag.clone(),
            payload: Box::new(erase_declared_identity(payload)),
        },
        TypeDescriptor::Tuple(items) => {
            TypeDescriptor::Tuple(items.iter().map(erase_declared_identity).collect())
        }
        TypeDescriptor::Struct(fields) => TypeDescriptor::Struct(
            fields
                .iter()
                .map(|(name, field)| (name.clone(), erase_declared_identity(field)))
                .collect(),
        ),
        TypeDescriptor::Enum(variants) => TypeDescriptor::Enum(
            variants
                .iter()
                .map(|(name, payload)| {
                    (
                        name.clone(),
                        payload
                            .as_ref()
                            .map(|payload| Box::new(erase_declared_identity(payload))),
                    )
                })
                .collect(),
        ),
        TypeDescriptor::Union(variants) => {
            TypeDescriptor::Union(variants.iter().map(erase_declared_identity).collect())
        }
        TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
            parameters: parameters.iter().map(erase_declared_identity).collect(),
            result: Box::new(erase_declared_identity(result)),
        },
        descriptor => descriptor.clone(),
    }
}

fn enum_variant_type(name: &str, payload: Option<&TypeDescriptor>) -> TypeDescriptor {
    let tag = TypeDescriptor::Atom(atom_from_name(name));
    payload.map_or(tag, |payload| TypeDescriptor::Tagged {
        tag: atom_from_name(name),
        payload: Box::new(payload.clone()),
    })
}

fn incompatibility_path(actual: &TypeDescriptor, expected: &TypeDescriptor) -> Option<ValuePath> {
    fn visit(actual: &TypeDescriptor, expected: &TypeDescriptor, path: &mut ValuePath) -> bool {
        match (actual, expected) {
            (TypeDescriptor::Any, _) | (_, TypeDescriptor::Any) => false,
            (TypeDescriptor::Struct(actual), TypeDescriptor::Struct(expected)) => {
                for (name, expected) in expected {
                    path.push(ValuePathSegment::Key(name.clone()));
                    let mismatch = actual
                        .get(name)
                        .is_none_or(|actual| visit(actual, expected, path));
                    if mismatch {
                        return true;
                    }
                    path.pop();
                }
                if let Some(name) = actual.keys().find(|name| !expected.contains_key(*name)) {
                    path.push(ValuePathSegment::Key(name.clone()));
                    return true;
                }
                false
            }
            (TypeDescriptor::Enum(actual), TypeDescriptor::Enum(expected)) => {
                for (name, expected) in expected {
                    path.push(ValuePathSegment::Key(name.clone()));
                    let mismatch = match (actual.get(name), expected) {
                        (Some(None), None) => false,
                        (Some(Some(actual)), Some(expected)) => visit(actual, expected, path),
                        _ => true,
                    };
                    if mismatch {
                        return true;
                    }
                    path.pop();
                }
                if let Some(name) = actual.keys().find(|name| !expected.contains_key(*name)) {
                    path.push(ValuePathSegment::Key(name.clone()));
                    return true;
                }
                false
            }
            (TypeDescriptor::Tuple(actual), TypeDescriptor::Tuple(expected)) => {
                if actual.len() != expected.len() {
                    return true;
                }
                for (index, (actual, expected)) in actual.iter().zip(expected).enumerate() {
                    path.push(ValuePathSegment::Index(index));
                    if visit(actual, expected, path) {
                        return true;
                    }
                    path.pop();
                }
                false
            }
            (TypeDescriptor::Array(actual), TypeDescriptor::Array(expected)) => {
                visit(actual, expected, path)
            }
            (TypeDescriptor::Dict(actual), TypeDescriptor::Dict(expected)) => {
                visit(actual, expected, path)
            }
            (TypeDescriptor::Struct(actual), TypeDescriptor::Dict(expected)) => {
                for (name, actual) in actual {
                    path.push(ValuePathSegment::Key(name.clone()));
                    if visit(actual, expected, path) {
                        return true;
                    }
                    path.pop();
                }
                false
            }
            (
                TypeDescriptor::Tagged {
                    tag: actual_tag,
                    payload: actual,
                },
                TypeDescriptor::Tagged {
                    tag: expected_tag,
                    payload: expected,
                },
            ) => actual_tag != expected_tag || visit(actual, expected, path),
            _ => !assignable(actual, expected),
        }
    }
    let mut path = Vec::new();
    visit(actual, expected, &mut path).then_some(path)
}

fn type_at_path<'a>(
    descriptor: &'a TypeDescriptor,
    path: &[ValuePathSegment],
) -> Option<&'a TypeDescriptor> {
    let mut descriptor = descriptor;
    for segment in path {
        descriptor = match (segment, descriptor) {
            (ValuePathSegment::Key(name), TypeDescriptor::Struct(fields)) => fields.get(name)?,
            (ValuePathSegment::Index(index), TypeDescriptor::Tuple(items)) => items.get(*index)?,
            (_, TypeDescriptor::Array(item) | TypeDescriptor::Dict(item)) => item,
            _ => return None,
        };
    }
    Some(descriptor)
}

fn display_type_path(path: &[ValuePathSegment]) -> String {
    path.iter()
        .map(|segment| match segment {
            ValuePathSegment::Key(name) => format!(".{name}"),
            ValuePathSegment::Index(index) => format!("[{index}]"),
        })
        .collect()
}

fn expression_location_at_path(
    expression: &Expr,
    path: &[ValuePathSegment],
) -> Option<crate::Location> {
    let mut expression = expression;
    for segment in path {
        expression = match (segment, &expression.value) {
            (ValuePathSegment::Key(name), ExprKind::Dict(fields)) => fields
                .iter()
                .find(|field| {
                    field
                        .value
                        .name
                        .as_ref()
                        .is_some_and(|field_name| field_name.value == *name)
                })
                .map(|field| &field.value.value)?,
            (ValuePathSegment::Index(index), ExprKind::Array(items))
            | (ValuePathSegment::Index(index), ExprKind::Tuple(items)) => items.get(*index)?,
            _ => return None,
        };
    }
    Some(expression.location)
}

fn atom_from_name(name: &str) -> Atom {
    match name {
        "None" => Atom::builtin(BuiltinAtom::None),
        "Some" => Atom::builtin(BuiltinAtom::Some),
        "Ok" => Atom::builtin(BuiltinAtom::Ok),
        "Err" => Atom::builtin(BuiltinAtom::Err),
        "True" => Atom::builtin(BuiltinAtom::True),
        "False" => Atom::builtin(BuiltinAtom::False),
        _ => Atom::named(name),
    }
}

fn frontend_error(source_name: &str, message: impl Into<String>) -> FrontendError {
    FrontendError::new(
        source_name,
        SourceLocation {
            offset: 0,
            line: 1,
            column: 1,
        },
        message,
    )
}
