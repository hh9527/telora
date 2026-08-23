#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PropertyOwnerKind {
    Ty(crate::ast::DeclaredInitializerKind),
    Field,
    Variant,
}

const PROPERTY_CAP_TYPE: u32 = 1 << 0;
const PROPERTY_CAP_STRUCT_TYPE: u32 = 1 << 1;
const PROPERTY_CAP_ENUM_TYPE: u32 = 1 << 2;
const PROPERTY_CAP_MEMBER: u32 = 1 << 3;
const PROPERTY_CAP_FIELD: u32 = 1 << 4;
const PROPERTY_CAP_VARIANT: u32 = 1 << 5;

const PROPERTY_PREVIOUS_BINDING: &str = "\0telora_property_previous";

fn intrinsic_property_marker(decorator: &crate::ast::Decorator) -> bool {
    matches!(
        &decorator.value.callee.value,
        ExprKind::Variable(name) if name.value == "property"
    )
}

fn reserved_property_marker(decorator: &crate::ast::Decorator) -> bool {
    intrinsic_property_marker(decorator)
}

fn property_capability(
    decorator: &crate::ast::Decorator,
    sources: &SourceDatabase,
) -> Result<u32, FrontendError> {
    if !decorator.value.configured || decorator.value.arguments.len() != 1 {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                "@property requires exactly one capability Atom",
                decorator.location,
            ),
        ));
    }
    let argument = &decorator.value.arguments[0];
    let ExprKind::Atom(capability) = &argument.value else {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error("@property capability must be an Atom", argument.location),
        ));
    };
    match capability.as_str() {
        "Type" => Ok(PROPERTY_CAP_TYPE),
        "StructType" => Ok(PROPERTY_CAP_STRUCT_TYPE),
        "EnumType" => Ok(PROPERTY_CAP_ENUM_TYPE),
        "Member" => Ok(PROPERTY_CAP_MEMBER),
        "Field" => Ok(PROPERTY_CAP_FIELD),
        "Variant" => Ok(PROPERTY_CAP_VARIANT),
        _ => Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!("unknown @property capability '{capability}"),
                argument.location,
            ),
        )),
    }
}

fn owner_capability(owner: PropertyOwnerKind) -> u32 {
    match owner {
        PropertyOwnerKind::Ty(crate::ast::DeclaredInitializerKind::Struct) => {
            PROPERTY_CAP_TYPE | PROPERTY_CAP_STRUCT_TYPE
        }
        PropertyOwnerKind::Ty(crate::ast::DeclaredInitializerKind::Enum) => {
            PROPERTY_CAP_TYPE | PROPERTY_CAP_ENUM_TYPE
        }
        PropertyOwnerKind::Field => PROPERTY_CAP_MEMBER | PROPERTY_CAP_FIELD,
        PropertyOwnerKind::Variant => PROPERTY_CAP_MEMBER | PROPERTY_CAP_VARIANT,
    }
}

fn configured_decorator_provider(decorator: &crate::ast::Decorator) -> Expr {
    if decorator.value.configured {
        located(
            ExprKind::Call {
                callee: Box::new(decorator.value.callee.clone()),
                arguments: decorator.value.arguments.clone(),
            },
            decorator.location,
        )
    } else {
        decorator.value.callee.clone()
    }
}

fn property_context_descriptor(kind: PropertyOwnerKind) -> TypeDescriptor {
    match kind {
        PropertyOwnerKind::Ty(_) => TypeDescriptor::Type,
        PropertyOwnerKind::Field => TypeDescriptor::Struct(BTreeMap::from([
            ("index".into(), TypeDescriptor::Int),
            ("name".into(), TypeDescriptor::String),
            ("owner".into(), TypeDescriptor::Type),
            ("ty".into(), TypeDescriptor::Type),
        ])),
        PropertyOwnerKind::Variant => TypeDescriptor::Struct(BTreeMap::from([
            ("index".into(), TypeDescriptor::Int),
            ("name".into(), TypeDescriptor::String),
            ("owner".into(), TypeDescriptor::Type),
            ("payload".into(), option_descriptor(TypeDescriptor::Type)),
        ])),
    }
}

fn decorator_property_descriptor(
    decorator: &crate::ast::Decorator,
    owner: PropertyOwnerKind,
    environment: &HashMap<String, TypeDescriptor>,
    sources: &SourceDatabase,
) -> Result<TypeDescriptor, FrontendError> {
    let mut provider = infer_expr_recorded(
        &decorator.value.callee,
        environment,
        &mut HashMap::new(),
    );
    if decorator.value.configured {
        let TypeDescriptor::Function { parameters, result } = provider else {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error("configured decorator target is not callable", decorator.location),
            ));
        };
        if parameters.len() != decorator.value.arguments.len() {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    format!(
                        "configured decorator expects {} argument(s), got {}",
                        parameters.len(),
                        decorator.value.arguments.len()
                    ),
                    decorator.location,
                ),
            ));
        }
        for (argument, expected) in decorator.value.arguments.iter().zip(&parameters) {
            let actual = infer_expr_recorded(argument, environment, &mut HashMap::new());
            if !contains_any_descriptor(&actual) && !assignable(&actual, expected) {
                return Err(FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!(
                            "decorator argument has type {}, which is not assignable to {}",
                            actual.display_name(),
                            expected.display_name()
                        ),
                        argument.location,
                    ),
                ));
            }
        }
        provider = *result;
    }
    let TypeDescriptor::Function { parameters, result } = provider else {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                "decorator must provide Fn(Ctx, Option(Property)) -> Property",
                decorator.location,
            ),
        ));
    };
    if parameters.len() != 2 {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                "decorator provider must accept exactly its context and previous property",
                decorator.location,
            ),
        ));
    }
    let expected_context = property_context_descriptor(owner);
    let accepts_context = match &parameters[0] {
        TypeDescriptor::Declared(declared) => assignable(&expected_context, &declared.body),
        parameter => assignable(&expected_context, parameter),
    };
    if !matches!(parameters[0], TypeDescriptor::Any)
        && !accepts_context
    {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!(
                    "decorator context {} is not assignable to {}",
                    expected_context.display_name(),
                    parameters[0].display_name()
                ),
                decorator.location,
            ),
        ));
    }
    let property = *result;
    let expected_previous = option_descriptor(property.clone());
    if parameters[1] != expected_previous {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!(
                    "decorator previous parameter must be {}, got {}",
                    expected_previous.display_name(),
                    parameters[1].display_name()
                ),
                decorator.location,
            ),
        ));
    }
    Ok(property)
}

fn property_call(
    decorator: &crate::ast::Decorator,
    context: Expr,
    location: crate::source::Location,
) -> Expr {
    located(
        ExprKind::Call {
            callee: Box::new(configured_decorator_provider(decorator)),
            arguments: vec![
                context,
                located(
                    ExprKind::Variable(located(
                        PROPERTY_PREVIOUS_BINDING.to_owned(),
                        location,
                    )),
                    location,
                ),
            ],
        },
        decorator.location,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_property_decorator(
    source_name: &str,
    decorator: &crate::ast::Decorator,
    owner: PropertyOwnerKind,
    context: Expr,
    previous: Option<Val>,
    tool_values: &BTreeMap<String, Val>,
    static_environment: &HashMap<String, TypeDescriptor>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<(TypeId, Val), FrontendError> {
    if reserved_property_marker(decorator) {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                "@property only declares capabilities on property carrier types",
                decorator.location,
            ),
        ));
    }
    let property_descriptor =
        decorator_property_descriptor(decorator, owner, static_environment, sources)?;
    if !matches!(property_descriptor, TypeDescriptor::Declared(_))
        || type_identity_is_symbolic(&property_descriptor)
    {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!(
                    "decorator result must be a concrete nominal property type, got {}",
                    property_descriptor.display_name()
                ),
                decorator.location,
            ),
        ));
    }
    let property_type = evaluator.canonical_type_id(&property_descriptor)?;
    if evaluator.property_attr_type() == Some(property_type) {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                "PropertyAttr is reserved for @property capability records",
                decorator.location,
            ),
        ));
    }
    let capabilities = evaluator.property_capabilities(property_type).map_err(|error| {
        FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(error.to_string(), decorator.location),
        )
    })?;
    if capabilities & owner_capability(owner) == 0 {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!(
                    "property type {} does not support this decorator target",
                    property_descriptor.display_name()
                ),
                decorator.location,
            ),
        ));
    }

    let mut environment = static_environment.clone();
    environment.insert(
        PROPERTY_PREVIOUS_BINDING.into(),
        option_descriptor(property_descriptor.clone()),
    );
    let call = property_call(decorator, context, decorator.location);
    let mut descriptors = HashMap::new();
    let inferred = infer_expr_recorded(&call, &environment, &mut descriptors);
    if inferred != property_descriptor {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!(
                    "decorator call inferred {}, expected {}",
                    inferred.display_name(),
                    property_descriptor.display_name()
                ),
                decorator.location,
            ),
        ));
    }
    let mut values = tool_values.clone();
    let previous = evaluator.previous_property_value(previous);
    values.insert(PROPERTY_PREVIOUS_BINDING.into(), previous);
    let property = evaluate_typed_tool_expression_silent(
        source_name,
        &call,
        &values,
        &descriptors,
        account,
        sources,
        evaluator,
    )?;
    if property.type_id() != Some(property_type) {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                "decorator result runtime witness does not match its static property type",
                decorator.location,
            ),
        ));
    }
    Ok((property_type, property))
}

fn declared_member_fields(binding: &Binding) -> Option<&[crate::ast::DictField]> {
    let ExprKind::Call { arguments, .. } = &binding.value.value.value else {
        return None;
    };
    let ExprKind::Dict(fields) = &arguments.get(1)?.value else {
        return None;
    };
    Some(fields)
}

fn owner_expression(binding: &Binding) -> Expr {
    located(
        ExprKind::Variable(binding.value.name.clone()),
        binding.value.name.location,
    )
}

fn context_field(name: &str, value: Expr, location: crate::source::Location) -> crate::ast::DictField {
    located(
        crate::ast::DictFieldKind {
            decorators: Vec::new(),
            name: Some(located(name.to_owned(), location)),
            value,
        },
        location,
    )
}

fn member_context(
    binding: &Binding,
    member: &crate::ast::DictField,
    index: u32,
    owner: PropertyOwnerKind,
) -> Expr {
    let location = member.location;
    let name = member
        .value
        .name
        .as_ref()
        .expect("declared member has a name");
    let mut fields = vec![
        context_field("owner", owner_expression(binding), location),
        context_field(
            "index",
            located(ExprKind::Int(i64::from(index)), location),
            location,
        ),
        context_field(
            "name",
            located(ExprKind::String(name.value.clone()), name.location),
            location,
        ),
    ];
    match owner {
        PropertyOwnerKind::Field => fields.push(context_field(
            "ty",
            member.value.value.clone(),
            location,
        )),
        PropertyOwnerKind::Variant => {
            let payload = if matches!(&member.value.value.value, ExprKind::Atom(tag) if tag == "None")
            {
                located(ExprKind::Atom("None".into()), location)
            } else {
                located(
                    ExprKind::Call {
                        callee: Box::new(located(ExprKind::Atom("Some".into()), location)),
                        arguments: vec![member.value.value.clone()],
                    },
                    location,
                )
            };
            fields.push(context_field("payload", payload, location));
        }
        PropertyOwnerKind::Ty(_) => unreachable!("type context is the owner expression"),
    }
    located(ExprKind::Dict(fields), location)
}

fn binding_has_member_decorators(binding: &Binding) -> bool {
    declared_member_fields(binding).is_some_and(|members| {
        members
            .iter()
            .any(|member| !member.value.decorators.is_empty())
    })
}

fn validate_decorated_binding(
    binding: &Binding,
    sources: &SourceDatabase,
) -> Result<(), FrontendError> {
    if binding.value.kind == BindingKind::Type
        && binding.value.declared_initializer.is_some()
        && binding.value.type_parameters.is_empty()
    {
        return Ok(());
    }
    Err(FrontendError::from_diagnostic(
        sources,
        Diagnostic::error(
            "decorators are only supported on concrete nominal struct or enum declarations",
            binding.location,
        ),
    ))
}

fn establish_property_markers(
    program: &Program,
    tool_values: &BTreeMap<String, Val>,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<(Option<TypeId>, Vec<(PropertyKey, Val)>), FrontendError> {
    let mut bootstrap_type = None;
    let mut properties = Vec::new();
    for binding in &program.value.body.value.bindings {
        let markers = binding
            .value
            .decorators
            .iter()
            .filter(|decorator| intrinsic_property_marker(decorator))
            .collect::<Vec<_>>();
        if markers.is_empty() {
            continue;
        }
        validate_decorated_binding(binding, sources)?;
        let target = tool_values[&binding.value.name.value];
        let target_type = evaluator.declared_type_id(target)?;
        let mut capabilities = 0;
        for decorator in &markers {
            capabilities |= property_capability(decorator, sources)?;
        }
        let bootstrap = binding.value.name.value == "PropertyAttr"
            && evaluator.property_attr_type().is_none();
        let marker_type = if bootstrap {
            target_type
        } else {
            evaluator.property_attr_type().ok_or_else(|| {
                FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        "@property requires the PropertyAttr bootstrap",
                        markers[0].location,
                    ),
                )
            })?
        };
        let key = PropertyKey::Ty {
                ty: target_type,
                property_ty: marker_type,
        };
        let value = evaluator.property_attr_value(marker_type, capabilities);
        if bootstrap {
            evaluator.establish_property_attr_type(target_type)?;
            bootstrap_type = Some(target_type);
        }
        evaluator.stage_property(key, value)?;
        properties.push((key, value));
    }
    Ok((bootstrap_type, properties))
}

#[allow(clippy::too_many_arguments)]
fn evaluate_declared_properties(
    source_name: &str,
    program: &Program,
    tool_values: &BTreeMap<String, Val>,
    static_environment: &HashMap<String, TypeDescriptor>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<(), FrontendError> {
    let (bootstrap_type, mut publication) =
        establish_property_markers(program, tool_values, sources, evaluator)?;
    for binding in &program.value.body.value.bindings {
        let type_decorators = binding
            .value
            .decorators
            .iter()
            .filter(|decorator| !intrinsic_property_marker(decorator))
            .collect::<Vec<_>>();
        if type_decorators.is_empty() && !binding_has_member_decorators(binding) {
            continue;
        }
        validate_decorated_binding(binding, sources)?;
        let target = tool_values
            .get(&binding.value.name.value)
            .copied()
            .ok_or_else(|| {
                FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error(
                        format!(
                            "decorated type {} has no sealed TypeDesc",
                            binding.value.name.value
                        ),
                        binding.location,
                    ),
                )
            })?;
        let target_type = evaluator.declared_type_id(target)?;
        let owner_kind = match binding.value.declared_initializer {
            Some(crate::ast::DeclaredInitializerKind::Struct) => PropertyOwnerKind::Field,
            Some(crate::ast::DeclaredInitializerKind::Enum) => PropertyOwnerKind::Variant,
            None => unreachable!("decorated binding was validated as nominal"),
        };
        let mut effective = BTreeMap::<PropertyKey, Val>::new();
        let mut members = declared_member_fields(binding)
            .expect("declared initializer has members")
            .iter()
            .collect::<Vec<_>>();
        members.sort_by(|left, right| {
            left.value
                .name
                .as_ref()
                .expect("declared member has a name")
                .value
                .cmp(
                    &right
                        .value
                        .name
                        .as_ref()
                        .expect("declared member has a name")
                        .value,
                )
        });
        for (index, member) in members.into_iter().enumerate() {
            let index = u32::try_from(index).map_err(|_| {
                frontend_error(source_name, "declared type has too many members")
            })?;
            for decorator in &member.value.decorators {
                if intrinsic_property_marker(decorator) {
                    return Err(FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            "@property only declares capabilities on property carrier types",
                            decorator.location,
                        ),
                    ));
                }
                let context = member_context(binding, member, index, owner_kind);
                let property_descriptor = decorator_property_descriptor(
                    decorator,
                    owner_kind,
                    static_environment,
                    sources,
                )?;
                let property_type = evaluator.canonical_type_id(&property_descriptor)?;
                let key = match owner_kind {
                    PropertyOwnerKind::Field => PropertyKey::Field {
                        ty: target_type,
                        member_index: index,
                        property_ty: property_type,
                    },
                    PropertyOwnerKind::Variant => PropertyKey::Variant {
                        ty: target_type,
                        member_index: index,
                        property_ty: property_type,
                    },
                    PropertyOwnerKind::Ty(_) => unreachable!(),
                };
                let previous = effective.get(&key).copied();
                let (actual_type, value) = evaluate_property_decorator(
                    source_name,
                    decorator,
                    owner_kind,
                    context,
                    previous,
                    tool_values,
                    static_environment,
                    account,
                    sources,
                    evaluator,
                )?;
                debug_assert_eq!(actual_type, property_type);
                effective.insert(key, value);
            }
        }
        for (key, value) in &effective {
            evaluator.stage_property(*key, *value)?;
        }

        for decorator in type_decorators {
            let property_descriptor = decorator_property_descriptor(
                decorator,
                PropertyOwnerKind::Ty(
                    binding
                        .value
                        .declared_initializer
                        .expect("decorated binding is nominal"),
                ),
                static_environment,
                sources,
            )?;
            let property_type = evaluator.canonical_type_id(&property_descriptor)?;
            let key = PropertyKey::Ty {
                ty: target_type,
                property_ty: property_type,
            };
            let previous = effective.get(&key).copied();
            let (actual_type, value) = evaluate_property_decorator(
                source_name,
                decorator,
                PropertyOwnerKind::Ty(
                    binding
                        .value
                        .declared_initializer
                        .expect("decorated binding is nominal"),
                ),
                owner_expression(binding),
                previous,
                tool_values,
                static_environment,
                account,
                sources,
                evaluator,
            )?;
            debug_assert_eq!(actual_type, property_type);
            effective.insert(key, value);
        }
        for (key, value) in &effective {
            evaluator.stage_property(*key, *value)?;
        }
        publication.extend(effective);
    }
    evaluator
        .publish_type_properties(bootstrap_type, &publication)
        .map_err(|error| {
            FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(error.to_string(), program.location),
            )
        })
}
