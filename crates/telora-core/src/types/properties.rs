#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PropertyOwnerKind {
    Ty(crate::ast::DeclaredInitializerKind),
    Field,
    Variant,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypePropertyEvidence {
    pub target: TypeDescriptor,
    pub property: TypeDescriptor,
    pub root: String,
}

fn type_value_descriptor(descriptor: &TypeDescriptor) -> Option<TypeDescriptor> {
    let TypeDescriptor::TypeOf(target) = descriptor else {
        return None;
    };
    Some((**target).clone())
}

const PROPERTY_CAP_TYPE: u32 = 1 << 0;
const PROPERTY_CAP_STRUCT_TYPE: u32 = 1 << 1;
const PROPERTY_CAP_ENUM_TYPE: u32 = 1 << 2;
const PROPERTY_CAP_MEMBER: u32 = 1 << 3;
const PROPERTY_CAP_FIELD: u32 = 1 << 4;
const PROPERTY_CAP_VARIANT: u32 = 1 << 5;

const PROPERTY_PREVIOUS_BINDING: &str = "\0telora_property_previous";

struct PreparedPropertyDecorator {
    descriptor: TypeDescriptor,
    plan: PreparedToolExpression,
}

#[derive(Default)]
struct PreparedPropertyPlans {
    capabilities: HashMap<crate::Location, PreparedToolExpression>,
    decorators: HashMap<crate::Location, PreparedPropertyDecorator>,
}

// These plans depend only on solved contracts and syntax, including the type of
// `previous`. Capability values and chained property values are execution inputs.
fn prepare_property_plans(
    program: &Program,
    contracts: &HashMap<crate::Location, TypeDescriptor>,
    environment: &HashMap<String, TypeDescriptor>,
    query: Option<crate::query::QueryContext>,
    sources: &SourceDatabase,
    context: &mut ToolInferenceContext<'_>,
) -> Result<PreparedPropertyPlans, FrontendError> {
    let mut plans = PreparedPropertyPlans::default();
    for binding in &program.value.body.value.bindings {
        for decorator in binding.value.decorators.iter().filter(|d| intrinsic_property_marker(d)) {
            validate_decorated_binding(binding, sources)?;
            if !decorator.value.configured || decorator.value.arguments.len() != 1 {
                return Err(FrontendError::from_diagnostic(sources, Diagnostic::error(
                    "@property requires exactly one PropertyTarget value", decorator.location)));
            }
            let argument = &decorator.value.arguments[0];
            let evidence = solve_tool_expression_types(argument, Some(&property_target_descriptor()),
                query.clone(), sources, context).map_err(|message|
                    FrontendError::from_diagnostic(sources, Diagnostic::error(
                        format!("@property requires PropertyTarget: {message}"), argument.location)))?;
            let plan = prepare_tool_execution(argument, evidence, sources, &mut context.types).map_err(|message|
                FrontendError::from_diagnostic(sources, Diagnostic::error(message, argument.location)))?;
            plans.capabilities.insert(decorator.location, plan);
        }
        let decorators = binding.value.decorators.iter()
            .filter(|d| !intrinsic_property_marker(d) && !intrinsic_check_marker(d)).collect::<Vec<_>>();
        if decorators.is_empty() && !binding_has_member_decorators(binding) { continue; }
        validate_decorated_binding(binding, sources)?;
        let kind = binding.value.declared_initializer.expect("validated nominal binding");
        let owner = match kind {
            crate::ast::DeclaredInitializerKind::Struct | crate::ast::DeclaredInitializerKind::Newtype => PropertyOwnerKind::Field,
            crate::ast::DeclaredInitializerKind::Enum => PropertyOwnerKind::Variant,
        };
        let mut members = declared_member_fields(binding).expect("validated nominal members").iter().collect::<Vec<_>>();
        members.sort_by_key(|member| member.value.name.as_ref().map(|name| &name.value));
        let mut prepare = |decorator: &crate::ast::Decorator, input| -> Result<(), FrontendError> {
            if intrinsic_property_marker(decorator) {
                return Err(FrontendError::from_diagnostic(sources, Diagnostic::error(
                    "@property only declares capabilities on property carrier types", decorator.location)));
            }
            let descriptor = contracts.get(&decorator.location).cloned().ok_or_else(||
                FrontendError::from_diagnostic(sources, Diagnostic::error(
                    "property decorator has no solved contract", decorator.location)))?;
            let call = property_call(decorator, input, decorator.location);
            let plan = prepare_property_call(&call, &descriptor, environment, query.clone(), sources, context)
                .map_err(|message| FrontendError::from_diagnostic(sources, Diagnostic::error(message, decorator.location)))?;
            plans.decorators.insert(decorator.location, PreparedPropertyDecorator { descriptor, plan });
            Ok(())
        };
        for (index, member) in members.into_iter().enumerate() {
            let index = u32::try_from(index).map_err(|_| FrontendError::from_diagnostic(sources,
                Diagnostic::error("declared type has too many members", binding.location)))?;
            for decorator in member.value.decorators.iter().filter(|d| !intrinsic_check_marker(d)) {
                prepare(decorator, member_context(binding, member, index, owner))?;
            }
        }
        for decorator in decorators {
            prepare(decorator, owner_expression(binding))?;
        }
    }
    Ok(plans)
}

fn intrinsic_property_marker(decorator: &crate::ast::Decorator) -> bool {
    matches!(
        &decorator.value.callee.value,
        ExprKind::Variable(name) if name.value == "property"
    )
}

fn property_capability(
    decorator: &crate::ast::Decorator,
    plan: &PreparedToolExpression,
    tool_values: &BTreeMap<String, Val>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<u32, FrontendError> {
    let argument = &decorator.value.arguments[0];
    let source_name = sources.get(argument.location.source).name.to_string();
    let value = evaluate_prepared_tool_expression(&source_name, tool_values,
        plan,
        account, sources, evaluator, false)?;
    let value = ValueRef::work(value, &evaluator.work, evaluator.main);
    let capability = value.as_atom().ok_or_else(|| FrontendError::from_diagnostic(sources,
        Diagnostic::error("@property must evaluate to a PropertyTarget value", argument.location)))?;
    match capability.as_ref() {
        "Type" => Ok(PROPERTY_CAP_TYPE),
        "StructType" => Ok(PROPERTY_CAP_STRUCT_TYPE),
        "EnumType" => Ok(PROPERTY_CAP_ENUM_TYPE),
        "Member" => Ok(PROPERTY_CAP_MEMBER),
        "Field" => Ok(PROPERTY_CAP_FIELD),
        "Variant" => Ok(PROPERTY_CAP_VARIANT),
        _ => Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                format!("unknown @property capability {capability:?}"),
                argument.location,
            ),
        )),
    }
}

fn owner_capability(owner: PropertyOwnerKind) -> u32 {
    match owner {
        PropertyOwnerKind::Ty(crate::ast::DeclaredInitializerKind::Struct | crate::ast::DeclaredInitializerKind::Newtype) => {
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

fn property_provider_result(
    decorator: &crate::ast::Decorator,
    owner: PropertyOwnerKind,
    provider: TypeDescriptor,
    sources: &SourceDatabase,
) -> Result<TypeDescriptor, FrontendError> {
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
    if !accepts_context {
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
                    ExprKind::Variable(located(PROPERTY_PREVIOUS_BINDING.to_owned(), location)),
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
    prepared: &PreparedPropertyDecorator,
    previous: Option<Val>,
    tool_values: &BTreeMap<String, Val>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<(TypeId, Val), FrontendError> {
    let property_descriptor = &prepared.descriptor;
    let property_type = evaluator.canonical_type_id(property_descriptor)?;
    if evaluator.property_attr_type() == Some(property_type) {
        return Err(FrontendError::from_diagnostic(
            sources,
            Diagnostic::error(
                "PropertyAttr is reserved for @property capability records",
                decorator.location,
            ),
        ));
    }
    let capabilities = evaluator
        .property_capabilities(property_type)
        .map_err(|error| {
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

    let mut values = ScopedToolBindings::new(tool_values);
    let previous = evaluator.previous_property_value(previous);
    values.insert(PROPERTY_PREVIOUS_BINDING.into(), previous);
    let property = evaluate_prepared_tool_expression(
        source_name, &values, &prepared.plan, account, sources, evaluator, false)?;
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

// Prepare the chained call without allocating its previous runtime value or
// accessing the VM. Restore lexical inputs on both successful and failed solves.
fn prepare_property_call(
    call: &Expr,
    property: &TypeDescriptor,
    environment: &dyn TypeEnvironment,
    query: Option<crate::query::QueryContext>,
    sources: &SourceDatabase,
    context: &mut ToolInferenceContext<'_>,
) -> Result<PreparedToolExpression, String> {
    let mut environment = ScopedTypeEnvironment::new(environment);
    environment.insert(PROPERTY_PREVIOUS_BINDING.into(), option_descriptor(property.clone()));
    let previous = context.scope_environment_inputs(call, &environment);
    let evidence = solve_tool_expression_types(call, Some(property), query, sources, context);
    context.restore_environment_inputs(previous);
    prepare_tool_execution(call, evidence?, sources, &mut context.types)
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

fn context_field(
    name: &str,
    value: Expr,
    location: crate::source::Location,
) -> crate::ast::DictField {
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
        PropertyOwnerKind::Field => {
            fields.push(context_field("ty", member.value.value.clone(), location))
        }
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
            .any(|member| member.value.decorators.iter().any(|decorator| !intrinsic_check_marker(decorator)))
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

fn declared_property_contracts(
    program: &Program,
    environment: &dyn TypeEnvironment,
    sources: &SourceDatabase,
    mut provider: impl FnMut(&crate::ast::Decorator, PropertyOwnerKind) -> Result<TypeDescriptor, FrontendError>,
) -> Result<Vec<(TypeDescriptor, TypeDescriptor)>, FrontendError> {
    let mut evidence = BTreeMap::new();
    for binding in &program.value.body.value.bindings {
        if !binding.value.decorators.iter().any(|decorator| !intrinsic_check_marker(decorator))
            && !binding_has_member_decorators(binding) {
            continue;
        }
        validate_decorated_binding(binding, sources)?;
        let target = environment.get(&binding.value.name.value)
            .and_then(type_value_descriptor)
            .ok_or_else(|| FrontendError::from_diagnostic(sources,
                Diagnostic::error("decorated type remains unknown", binding.location)))?;
        for decorator in binding.value.decorators.iter().filter(|decorator| !intrinsic_check_marker(decorator)) {
            let property = if intrinsic_property_marker(decorator) {
                environment.get("PropertyAttr").and_then(type_value_descriptor)
                    .ok_or_else(|| FrontendError::from_diagnostic(sources, Diagnostic::error(
                        "@property requires the PropertyAttr bootstrap", decorator.location,
                    )))?
            } else {
                provider(
                    decorator,
                    PropertyOwnerKind::Ty(binding.value.declared_initializer.expect("nominal declaration")),
                )?
            };
            if !matches!(property, TypeDescriptor::Declared(_)) || type_identity_is_symbolic(&property) {
                return Err(FrontendError::from_diagnostic(sources, Diagnostic::error(
                    format!("decorator result must be a concrete nominal property type, got {}", property.display_name()),
                    decorator.location,
                )));
            }
            let key = (TypeExprId::from_descriptor(&target), TypeExprId::from_descriptor(&property));
            evidence.insert(key, (target.clone(), property));
        }
        if let Some(members) = declared_member_fields(binding) {
            let owner = match binding.value.declared_initializer {
                Some(crate::ast::DeclaredInitializerKind::Enum) => PropertyOwnerKind::Variant,
                _ => PropertyOwnerKind::Field,
            };
            for member in members {
                for decorator in member.value.decorators.iter().filter(|d| !intrinsic_check_marker(d)) {
                    if intrinsic_property_marker(decorator) {
                        return Err(FrontendError::from_diagnostic(sources, Diagnostic::error(
                            "@property only declares capabilities on property carrier types", decorator.location)));
                    }
                    provider(decorator, owner)?;
                }
            }
        }
    }
    Ok(evidence.into_values().collect())
}

#[cfg(test)]
mod property_contract_tests {
    use super::*;

    #[test]
    fn repeated_property_declarations_produce_one_static_presence_record() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("static-property-presence", "@tag @tag type Record = struct {value: Int};");
        let program = parse_registered(&sources, source).program.unwrap();
        let nominal = |local, name: &str| TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: crate::value::DeclaredTypeId::concrete(crate::ModuleId::ANONYMOUS, local),
            name: name.into(), body: Arc::new(TypeDescriptor::Struct(BTreeMap::new())),
        });
        let target = nominal(80, "Record");
        let property = nominal(81, "Label");
        let environment = HashMap::from([
            ("Record".into(), TypeDescriptor::TypeOf(Box::new(target.clone()))),
            ("tag".into(), TypeDescriptor::Function {
                parameters: vec![TypeDescriptor::Type, option_descriptor(property.clone())],
                result: Box::new(property.clone()),
            }),
        ]);
        let contracts = declared_property_contracts(&program, &environment, &sources,
            |decorator, owner| property_provider_result(decorator, owner, environment["tag"].clone(), &sources)).unwrap();
        assert_eq!(contracts, vec![(target, property)]);
    }
}

fn declared_property_evidence(
    module_id: crate::ModuleId,
    contracts: Vec<(TypeDescriptor, TypeDescriptor)>,
) -> Vec<TypePropertyEvidence> {
    contracts.into_iter().enumerate().map(|(index, (target, property))| {
        TypePropertyEvidence { target, property,
            root: format!("\0type_property:{}:{index}", module_id.raw()) }
    }).collect()
}

fn solve_declared_property_contracts(
    program: &Program,
    environment: &dyn TypeEnvironment,
    inference: &mut GenericInference<'_>,
    sources: &SourceDatabase,
) -> Result<Vec<(TypeDescriptor, TypeDescriptor)>, FrontendError> {
    declared_property_contracts(program, environment, sources, |decorator, owner| {
        let expression = configured_decorator_provider(decorator);
        let provider = inference.infer(&expression, environment, None).map_err(|message| {
            FrontendError::from_diagnostic(sources,
                inference.take_failure_diagnostic(decorator.location, message, None))
        })?;
        let property = property_provider_result(decorator, owner, inference.normalize(&provider), sources)?;
        if !matches!(property, TypeDescriptor::Declared(_)) || type_identity_is_symbolic(&property) {
            return Err(FrontendError::from_diagnostic(sources, Diagnostic::error(
                format!("decorator result must be a concrete nominal property type, got {}", property.display_name()),
                decorator.location,
            )));
        }
        inference.property_contracts.insert(decorator.location, property.clone());
        Ok(property)
    })
}

fn establish_property_markers(
    program: &Program,
    plans: &PreparedPropertyPlans,
    tool_values: &BTreeMap<String, Val>,
    static_environment: &HashMap<String, TypeDescriptor>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<
    (
        Option<TypeId>,
        Vec<(PropertyKey, Val)>,
        BTreeMap<PropertyKey, (TypeDescriptor, TypeDescriptor)>,
    ),
    FrontendError,
> {
    let mut bootstrap_type = None;
    let mut properties = Vec::new();
    let mut descriptors = BTreeMap::new();
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
        let target_descriptor = static_environment
            .get(&binding.value.name.value)
            .and_then(type_value_descriptor)
            .expect("property carrier has a concrete Type descriptor");
        let mut capabilities = 0;
        for decorator in &markers {
            capabilities |= property_capability(decorator, &plans.capabilities[&decorator.location],
                tool_values, account, sources, evaluator)?;
        }
        let bootstrap =
            binding.value.name.value == "PropertyAttr" && evaluator.property_attr_type().is_none();
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
        let marker_descriptor = if bootstrap {
            target_descriptor.clone()
        } else {
            static_environment
                .get("PropertyAttr")
                .and_then(type_value_descriptor)
                .expect("PropertyAttr has a concrete Type descriptor")
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
        descriptors.insert(key, (target_descriptor, marker_descriptor));
    }
    Ok((bootstrap_type, properties, descriptors))
}

#[allow(clippy::too_many_arguments)]
fn evaluate_declared_properties(
    source_name: &str,
    program: &Program,
    plans: &PreparedPropertyPlans,
    expected: &[TypePropertyEvidence],
    tool_values: &BTreeMap<String, Val>,
    static_environment: &HashMap<String, TypeDescriptor>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<Vec<(TypePropertyEvidence, PersistentValue)>, FrontendError> {
    let (bootstrap_type, mut publication, mut evidence_descriptors) = establish_property_markers(
        program,
        plans,
        tool_values,
        static_environment,
        account,
        sources,
        evaluator,
    )?;
    for binding in &program.value.body.value.bindings {
        let type_decorators = binding
            .value
            .decorators
            .iter()
            .filter(|decorator| !intrinsic_property_marker(decorator) && !intrinsic_check_marker(decorator))
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
        let target_descriptor = static_environment
            .get(&binding.value.name.value)
            .and_then(type_value_descriptor)
            .expect("decorated type has a concrete Type descriptor");
        let owner_kind = match binding.value.declared_initializer {
            Some(crate::ast::DeclaredInitializerKind::Struct | crate::ast::DeclaredInitializerKind::Newtype) => PropertyOwnerKind::Field,
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
            let index = u32::try_from(index)
                .map_err(|_| frontend_error(source_name, "declared type has too many members"))?;
            for decorator in &member.value.decorators {
                if intrinsic_check_marker(decorator) { continue; }
                if intrinsic_property_marker(decorator) {
                    return Err(FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            "@property only declares capabilities on property carrier types",
                            decorator.location,
                        ),
                    ));
                }
                let prepared = &plans.decorators[&decorator.location];
                let property_type = evaluator.canonical_type_id(&prepared.descriptor)?;
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
                    prepared,
                    previous,
                    tool_values,
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
            let prepared = &plans.decorators[&decorator.location];
            let property_type = evaluator.canonical_type_id(&prepared.descriptor)?;
            let key = PropertyKey::Ty {
                ty: target_type,
                property_ty: property_type,
            };
            evidence_descriptors.insert(
                key,
                (target_descriptor.clone(), prepared.descriptor.clone()),
            );
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
                prepared,
                previous,
                tool_values,
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
        })?;
    if evidence_descriptors.len() != expected.len() {
        return Err(frontend_error(source_name, "materialized property set differs from solved contracts"));
    }
    expected
        .iter()
        .map(|evidence| {
            let ty = evaluator.canonical_type_id(&evidence.target)?;
            let property_ty = evaluator.canonical_type_id(&evidence.property)?;
            if !evidence_descriptors.contains_key(&PropertyKey::Ty { ty, property_ty }) {
                return Err(frontend_error(source_name, "solved property contract was not materialized"));
            }
            let value = evaluator
                .persistent_type_property(ty, property_ty)
                .expect("published type property is present in Main world");
            Ok((evidence.clone(), value))
        })
        .collect()
}
