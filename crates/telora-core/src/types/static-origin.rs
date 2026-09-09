// Source use sites are separate from canonical type identity. This adapter
// projects their origins onto legacy metadata without executing a type expression.
#[cfg(test)]
mod static_origin_tests {
    use super::*;

    #[test]
    fn type_references_reuse_metadata_without_changing_shared_origins() {
        let mut sources = SourceDatabase::default();
        let prelude = sources.add("prelude", "Int");
        let original = crate::Location::from_usize(prelude, 0..3).unwrap();
        let source = sources.add("origins", "type Values = Array(Int);");
        let program = parse_registered(&sources, source).program.unwrap();
        let expression = &program.value.body.value.bindings[0].value.value;
        let mut heap = Heap::work();
        let integer = heap
            .type_descriptor_value(None, &TypeDescriptor::Int)
            .unwrap()
            .with_loc(Some(original));
        let values = BTreeMap::from([("Int".into(), integer)]);
        let array = heap
            .type_descriptor_value_with_origins(
                None,
                &TypeDescriptor::Array(Box::new(TypeDescriptor::Int)),
                Some(&|heap, path| {
                    static_type_origin(
                        expression,
                        path,
                        &values,
                        &HeapView {
                            current: heap,
                            background: None,
                        },
                    )
                }),
            )
            .unwrap();
        let crate::heap::DecodedValue::Dict(array) = array.value() else {
            panic!("array metadata");
        };
        let item = HeapView {
            current: &heap,
            background: None,
        }
        .dict_get_text(array, "item")
        .unwrap()
        .unwrap();
        assert_eq!(item.value(), integer.value(), "metadata object is reused");
        assert_ne!(item.loc(), integer.loc());
        assert_eq!(integer.loc(), Some(original));
    }

    #[test]
    fn symbolic_parameter_does_not_reuse_an_outer_type_reference() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("origins", "type T = Int; type Box(T) = struct {item: T};");
        let program = parse_registered(&sources, source).program.unwrap();
        let expression = &program.value.body.value.bindings[1].value.value;
        let mut heap = Heap::work();
        let integer = heap
            .type_descriptor_value(None, &TypeDescriptor::Int)
            .unwrap();
        let values = BTreeMap::from([("T".into(), integer)]);
        let body = TypeDescriptor::Struct(BTreeMap::from([(
            "item".into(),
            TypeDescriptor::Bound(TypeParameterId(0)),
        )]));
        let value = heap
            .type_descriptor_value_with_origins(
                None,
                &body,
                Some(&|heap, path| {
                    static_type_origin(
                        expression,
                        path,
                        &values,
                        &HeapView {
                            current: heap,
                            background: None,
                        },
                    )
                }),
            )
            .unwrap();
        assert_eq!(
            decode_type_ref(ValueRef::work(value, &heap, &Heap::main()), "Type").unwrap(),
            body
        );
    }

    #[test]
    fn shared_field_types_and_aliases_retain_distinct_use_sites() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("origins", "type Action = struct {first: Fn(Int) -> Int, second: Fn(Int) -> Int}; type Alias = Array(Action);");
        let program = parse_registered(&sources, source).program.unwrap();
        let body = &program.value.body.value.bindings[0].value.value;
        let alias = &program.value.body.value.bindings[1].value.value;
        let function = TypeDescriptor::Function {
            parameters: vec![TypeDescriptor::Int],
            result: Box::new(TypeDescriptor::Int),
        };
        let action = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 49),
            name: "Action".into(),
            body: Arc::new(TypeDescriptor::Struct(BTreeMap::from([
                ("first".into(), function.clone()),
                ("second".into(), function),
            ]))),
        });
        let mut heap = Heap::work();
        let mut values = BTreeMap::new();
        let value = heap
            .type_descriptor_value_with_origins(
                None,
                &action,
                Some(&|heap, path| {
                    static_type_origin(
                        body,
                        path,
                        &values,
                        &HeapView {
                            current: heap,
                            background: None,
                        },
                    )
                }),
            )
            .unwrap();
        values.insert("Action".into(), value);
        let value = heap
            .type_descriptor_value_with_origins(
                None,
                &TypeDescriptor::Array(Box::new(action)),
                Some(&|heap, path| {
                    static_type_origin(
                        alias,
                        path,
                        &values,
                        &HeapView {
                            current: heap,
                            background: None,
                        },
                    )
                }),
            )
            .unwrap();
        let view = HeapView {
            current: &heap,
            background: None,
        };
        let first = metadata_type_origin(
            value,
            &["item".into(), "body".into(), "fields/first".into()],
            None,
            &values,
            &view,
        )
        .unwrap();
        let second = metadata_type_origin(
            value,
            &["item".into(), "body".into(), "fields/second".into()],
            None,
            &values,
            &view,
        )
        .unwrap();
        assert_ne!(first, second);
        for location in [first, second] {
            assert_eq!(
                sources.get(location.source).slice(location).as_deref(),
                Some("Fn(Int) -> Int")
            );
        }
    }

    #[test]
    fn family_application_retains_definition_and_argument_origins() {
        let mut sources = SourceDatabase::default();
        let source = sources.add(
            "origins",
            "type Box(T) = struct {item: T}; type Action = Box(Fn(Int) -> Int);",
        );
        let program = parse_registered(&sources, source).program.unwrap();
        let body = &program.value.body.value.bindings[0].value.value;
        let application = &program.value.body.value.bindings[1].value.value;
        let mut heap = Heap::work();
        let mut values = BTreeMap::new();
        let template = heap
            .type_descriptor_value_with_origins(
                None,
                &TypeDescriptor::Struct(BTreeMap::from([(
                    "item".into(),
                    TypeDescriptor::Bound(TypeParameterId(0)),
                )])),
                Some(&|heap, path| {
                    static_type_origin(
                        body,
                        path,
                        &values,
                        &HeapView {
                            current: heap,
                            background: None,
                        },
                    )
                }),
            )
            .unwrap();
        let family = heap.native_closure(
            NativeFunction::new("type-family.apply", 1, native_apply_type_family),
            vec![template],
        );
        values.insert("Box".into(), family);
        let descriptor = TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, 50),
            name: "Box".into(),
            body: Arc::new(TypeDescriptor::Struct(BTreeMap::from([(
                "item".into(),
                TypeDescriptor::Function {
                    parameters: vec![TypeDescriptor::Int],
                    result: Box::new(TypeDescriptor::Int),
                },
            )]))),
        });
        let value = heap
            .type_descriptor_value_with_origins(
                None,
                &descriptor,
                Some(&|heap, path| {
                    static_type_origin(
                        application,
                        path,
                        &values,
                        &HeapView {
                            current: heap,
                            background: None,
                        },
                    )
                }),
            )
            .unwrap();
        let view = HeapView {
            current: &heap,
            background: None,
        };
        let field = metadata_type_origin(
            value,
            &["body".into(), "fields/item".into()],
            None,
            &values,
            &view,
        )
        .unwrap();
        assert_eq!(sources.get(field.source).slice(field).as_deref(), Some("T"));
        let parameter = metadata_type_origin(
            value,
            &["body".into(), "fields/item".into(), "parameters/0".into()],
            None,
            &values,
            &view,
        )
        .unwrap();
        assert_eq!(
            sources.get(parameter.source).slice(parameter).as_deref(),
            Some("Int")
        );
    }
}

fn static_type_reference(
    expression: &Expr,
    values: &dyn ToolBindings,
    view: &HeapView<'_>,
) -> Option<Val> {
    use crate::heap::DecodedValue;
    match &expression.value {
        ExprKind::Variable(name) => values.get(&name.value).copied(),
        ExprKind::Field { receiver, field } => {
            match static_type_reference(receiver, values, view)?.value() {
                DecodedValue::Module(handle) => view.module_get_text(handle, &field.value).ok()?,
                DecodedValue::Dict(handle) => view.dict_get_text(handle, &field.value).ok()?,
                _ => None,
            }
        }
        _ => None,
    }
}

fn static_type_origin(
    expression: &Expr,
    path: &[String],
    values: &dyn ToolBindings,
    view: &HeapView<'_>,
) -> Option<crate::heap::TypeMetadataOrigin> {
    use crate::heap::{DecodedValue, Object, RuntimePrototype};
    let expression = match &expression.value {
        ExprKind::TypeSyntax(inner) => return static_type_origin(inner, path, values, view),
        _ => expression,
    };
    let Some((edge, remaining)) = path.split_first() else {
        return Some((
            expression.location,
            static_type_reference(expression, values, view),
        ));
    };
    if let Some(value) = static_type_reference(expression, values, view) {
        return metadata_type_origin(value, path, None, values, view)
            .map(|location| (location, None));
    }
    let ExprKind::Call { callee, arguments } = &expression.value else {
        return None;
    };
    if let Some(value) = static_type_reference(callee, values, view)
        && let DecodedValue::Func(handle) = value.value()
        && let Object::Closure {
            prototype: RuntimePrototype::Native(function),
            upvalues,
            ..
        } = view.object(handle).ok()?
        && function.name() == "type-family.apply"
    {
        return metadata_type_origin(*upvalues.first()?, path, Some(arguments), values, view)
            .map(|location| (location, None));
    }
    if edge == "body" {
        return static_type_origin(expression, remaining, values, view);
    }
    let ExprKind::Variable(callee) = &callee.value else {
        return None;
    };
    let child = match (callee.value.as_str(), edge.as_str()) {
        ("Array" | "Dict", "item") | ("TypeOf", "instance") => arguments.first()?,
        ("Func" | "\0telora_function_type", "result") => arguments.get(1)?,
        ("Func" | "\0telora_function_type", edge) if edge.starts_with("parameters/") => {
            let ExprKind::Array(items) = &arguments.first()?.value else {
                return None;
            };
            items.get(edge.strip_prefix("parameters/")?.parse::<usize>().ok()?)?
        }
        ("Tuple" | "\0telora_tuple_type", edge) if edge.starts_with("items/") => {
            let ExprKind::Array(items) = &arguments.first()?.value else {
                return None;
            };
            items.get(edge.strip_prefix("items/")?.parse::<usize>().ok()?)?
        }
        ("\0telora_struct" | "\0telora_enum" | "\0telora_newtype", edge) => {
            let ExprKind::Dict(fields) = &arguments.get(1)?.value else {
                return None;
            };
            let name = edge.split_once('/').map_or(edge, |(_, name)| name);
            &fields
                .iter()
                .find(|field| {
                    field
                        .value
                        .name
                        .as_ref()
                        .is_some_and(|field| field.value == name)
                })?
                .value
                .value
        }
        _ => return None,
    };
    static_type_origin(child, remaining, values, view)
}

fn metadata_type_origin(
    value: Val,
    path: &[String],
    arguments: Option<&[Expr]>,
    values: &dyn ToolBindings,
    view: &HeapView<'_>,
) -> Option<crate::Location> {
    use crate::heap::{DecodedValue, Object};
    let Some((edge, remaining)) = path.split_first() else {
        return value.loc();
    };
    if let DecodedValue::DeclaredType(handle) | DecodedValue::SymbolicType(handle) = value.value() {
        let body = match view.object(handle).ok()? {
            Object::DeclaredType { body, .. } | Object::SymbolicType { body, .. } => *body,
            _ => return None,
        };
        return metadata_type_origin(
            body,
            if edge == "body" { remaining } else { path },
            arguments,
            values,
            view,
        );
    }
    if edge == "body" {
        return metadata_type_origin(value, remaining, arguments, values, view);
    }
    let DecodedValue::Dict(handle) = value.value() else {
        return None;
    };
    let kind = view.dict_get_text(handle, "kind").ok()??;
    if view
        .atom_text(kind)
        .ok()
        .flatten()
        .is_some_and(|kind| kind == "Bound")
    {
        let parameter = view.dict_get_text(handle, "parameter").ok()??;
        let DecodedValue::Int(index) = parameter.value() else {
            return None;
        };
        return static_type_origin(
            arguments?.get(usize::try_from(index).ok()?)?,
            path,
            values,
            view,
        )
        .map(|(location, _)| location);
    }
    let child = if let Some((field, index)) = edge.split_once('/') {
        let container = view.dict_get_text(handle, field).ok()??;
        match container.value() {
            DecodedValue::Dict(handle) => view.dict_get_text(handle, index).ok()??,
            DecodedValue::Array(handle) => {
                let Object::Array(items) = view.object(handle).ok()? else {
                    return None;
                };
                *items.get(index.parse::<usize>().ok()?)?
            }
            _ => return None,
        }
    } else {
        view.dict_get_text(handle, edge).ok()??
    };
    metadata_type_origin(child, remaining, arguments, values, view)
}

impl ToolEvaluator<'_> {
    fn descriptor_with_origins(
        &mut self,
        descriptor: &TypeDescriptor,
        expression: &Expr,
        values: &dyn ToolBindings,
    ) -> Result<Val, FrontendError> {
        let main = &*self.main;
        self.work
            .type_descriptor_value_with_origins(
                Some(main),
                descriptor,
                Some(&|work, path| {
                    static_type_origin(
                        expression,
                        path,
                        values,
                        &HeapView {
                            current: work,
                            background: Some(main),
                        },
                    )
                }),
            )
            .map_err(|error| frontend_error("<tool-stage>", error.to_string()))
    }
}
