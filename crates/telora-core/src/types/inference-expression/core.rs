impl<'a> GenericInference<'a> {
    fn infer(
        &mut self,
        expression: &Expr,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        let constructs_declared_value = expression_constructs_declared_value(expression);
        let expected_declared = expected.and_then(|expected| {
            let TypeDescriptor::Declared(declared) = self.expose_named(expected) else {
                return None;
            };
            Some(declared)
        });
        let structural_expected = expected_declared
            .as_ref()
            .filter(|_| constructs_declared_value)
            .map(|declared| declared.body.as_ref());
        let mut result =
            self.infer_inner(expression, environment, structural_expected.or(expected));
        if let Err(message) = &result
            && self.enum_failure.is_none()
            && let Some(declared) = expected_declared.as_ref()
            && let Some((failure, replacement)) =
                self.direct_enum_failure(expression, declared, message)
        {
            self.enum_failure = Some(failure);
            result = Err(replacement);
        }
        if result.is_err() && self.failure_location.is_none() {
            self.failure_location = Some(
                self.enum_failure
                    .as_ref()
                    .filter(|failure| failure.kind == EnumInferenceFailureKind::MissingContext)
                    .map_or(expression.location, |_| {
                        self.narrow_value_origin(expression)
                    }),
            );
        }
        result.map(|inferred| {
            let Some(declared) = expected_declared else {
                return inferred;
            };
            if self
                .declared_identity(&inferred)
                .is_some_and(|actual| actual == declared.id)
            {
                return inferred;
            }
            if !constructs_declared_value && inferred != *declared.body {
                return inferred;
            }
            let declared = TypeDescriptor::Declared(declared);
            self.records.insert(expression.location, declared.clone());
            declared
        })
    }

    fn infer_authored_boundary(
        &mut self,
        expression: &Expr,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        let authored = expected.and_then(|expected| {
            self.authored_expression_contract(expression, environment)
                .map(|actual| (actual, self.resolve(expected)))
        });
        let inferred = self.infer(expression, environment, expected)?;
        if let Some((actual, expected)) = authored
            && narrows_any(&self.resolve(&actual), &expected)
        {
            return Err(format!(
                "cannot narrow {} to {} without cast!",
                actual.display_name(),
                expected.display_name()
            ));
        }
        Ok(inferred)
    }

    fn authored_expression_contract(
        &self,
        expression: &Expr,
        environment: &HashMap<String, TypeDescriptor>,
    ) -> Option<TypeDescriptor> {
        match &expression.value {
            ExprKind::Variable(name) => {
                let contract = self
                    .scheme(&name.value)
                    .map(|scheme| scheme.body)
                    .or_else(|| environment.get(&name.value).cloned())?;
                (self.reference_has_authored_contract(expression)
                    && contains_any_descriptor(&contract))
                .then_some(contract)
            }
            ExprKind::Field { .. } => self.explicit_scheme(expression).map(|scheme| scheme.body),
            ExprKind::Call { callee, .. } => {
                let callee_contract = self
                    .explicit_scheme(callee)
                    .map(|scheme| scheme.body)
                    .or_else(|| match &callee.value {
                        ExprKind::Variable(name) => environment.get(&name.value).cloned(),
                        _ => None,
                    })?;
                match callee_contract {
                    TypeDescriptor::Function { result, .. }
                        if !contains_any_descriptor(&result)
                            || self.reference_has_authored_contract(callee) =>
                    {
                        Some(*result)
                    }
                    _ => None,
                }
            }
            ExprKind::TypeAscription { target, .. } => {
                self.local_annotations.get(&target.location).cloned()
            }
            _ => None,
        }
    }

    fn reference_has_authored_contract(&self, expression: &Expr) -> bool {
        if let ExprKind::Field { receiver, .. } = &expression.value
            && let ExprKind::Variable(module) = &receiver.value
            && self.external_interfaces.contains_key(&module.value)
        {
            return true;
        }
        if let ExprKind::TypeApply { callee, .. } = &expression.value {
            return self.reference_has_authored_contract(callee);
        }
        self.hir
            .expression_ids_at(expression.location)
            .filter_map(|id| self.hir.expression(id))
            .filter_map(|expression| expression.reference)
            .filter_map(|id| self.hir.reference(id))
            .any(|reference| match reference.resolution {
                HirResolution::External => true,
                HirResolution::Definition(id) => {
                    self.hir.definition(id).is_some_and(|definition| {
                        self.authored_any_definitions.contains(&definition.location)
                    })
                }
                HirResolution::Unresolved => false,
            })
    }

    fn callee_has_runtime_boundary(&self, callee: &Expr) -> bool {
        if let ExprKind::Field { receiver, .. } = &callee.value
            && let ExprKind::Variable(module) = &receiver.value
            && self.external_interfaces.contains_key(&module.value)
        {
            return true;
        }
        if let ExprKind::TypeApply { callee, .. } = &callee.value {
            return self.callee_has_runtime_boundary(callee);
        }
        self.hir
            .expression_ids_at(callee.location)
            .filter_map(|id| self.hir.expression(id))
            .filter_map(|expression| expression.reference)
            .filter_map(|id| self.hir.reference(id))
            .any(|reference| match reference.resolution {
                HirResolution::External => true,
                HirResolution::Definition(id) => {
                    self.hir.definition(id).is_some_and(|definition| {
                        matches!(
                            definition.kind,
                            HirDefinitionKind::Import | HirDefinitionKind::Native
                        )
                    })
                }
                HirResolution::Unresolved => false,
            })
    }

    fn infer_inner(
        &mut self,
        expression: &Expr,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        if let Some(query) = &self.query {
            query.check().map_err(|error| error.to_string())?;
        }
        let inferred = match &expression.value {
            ExprKind::Variable(name) => self.scheme(&name.value).map_or_else(
                || {
                    environment
                        .get(&name.value)
                        .cloned()
                        .unwrap_or(TypeDescriptor::Any)
                },
                |scheme| self.instantiate(&scheme),
            ),
            ExprKind::Int(_) => TypeDescriptor::Int,
            ExprKind::Float(_) => TypeDescriptor::Float,
            ExprKind::String(_) => TypeDescriptor::String,
            ExprKind::InterpolatedString(parts) => {
                for part in parts {
                    if let StringPartKind::Expression(expression) = &part.value {
                        self.infer(expression, environment, None)?;
                    }
                }
                TypeDescriptor::String
            }
            ExprKind::Bytes(_) => TypeDescriptor::Bytes,
            ExprKind::Atom(name) => TypeDescriptor::Atom(atom_from_name(name)),
            ExprKind::Array(items) => {
                let item_expected = match expected.map(|ty| self.resolve(ty)) {
                    Some(TypeDescriptor::Array(item))
                        if items.is_empty()
                            || !matches!(self.resolve(&item), TypeDescriptor::Inference(_)) =>
                    {
                        Some(*item)
                    }
                    _ => None,
                };
                let mut item_types = Vec::new();
                for item in items {
                    if let ExprKind::Spread(operand) = &item.value {
                        let spread_expected = item_expected
                            .as_ref()
                            .map(|item| TypeDescriptor::Array(Box::new(item.clone())));
                        let spread = self.infer(operand, environment, spread_expected.as_ref())?;
                        let resolved = self.resolve(&spread);
                        let TypeDescriptor::Array(spread_item) = resolved else {
                            return Err(format!(
                                "array spread requires Array, found {}",
                                resolved.display_name()
                            ));
                        };
                        item_types.push(*spread_item);
                    } else {
                        item_types.push(self.infer(item, environment, item_expected.as_ref())?);
                    }
                }
                let item = if let Some(expected) = item_expected {
                    expected
                } else if items.is_empty() && self.delayed_initializer_depth > 0 {
                    self.fresh_variable()
                } else {
                    join_all_types(item_types)
                };
                TypeDescriptor::Array(Box::new(item))
            }
            ExprKind::Spread(operand) => self.infer(operand, environment, expected)?,
            ExprKind::Tuple(items) => {
                let item_expected = match expected.map(|ty| self.resolve(ty)) {
                    Some(TypeDescriptor::Tuple(expected_items))
                        if expected_items.len() == items.len() =>
                    {
                        expected_items
                    }
                    _ => Vec::new(),
                };
                TypeDescriptor::Tuple(
                    items
                        .iter()
                        .enumerate()
                        .map(|(index, item)| {
                            self.infer(item, environment, item_expected.get(index))
                        })
                        .collect::<Result<_, _>>()?,
                )
            }
            ExprKind::Dict(fields) => {
                let has_spread = fields.iter().any(|field| field.value.name.is_none());
                let metadata_expected = expected
                    .map(|ty| self.resolve(ty))
                    .filter(|ty| matches!(ty, TypeDescriptor::Type | TypeDescriptor::TypeOf(_)));
                if let Some(metadata_expected) = metadata_expected {
                    if has_spread {
                        return Err("Dict spread is not valid in type metadata".into());
                    }
                    for field in fields {
                        self.infer(&field.value.value, environment, None)?;
                    }
                    metadata_expected
                } else if has_spread {
                    let item_expected = match expected.map(|ty| self.resolve(ty)) {
                        Some(TypeDescriptor::Dict(item)) => Some(*item),
                        _ => None,
                    };
                    let mut item_types = Vec::new();
                    for field in fields {
                        if field.value.name.is_none() {
                            let ExprKind::Spread(operand) = &field.value.value.value else {
                                return Err("invalid Dict spread entry".into());
                            };
                            let spread_expected = item_expected
                                .as_ref()
                                .map(|item| TypeDescriptor::Dict(Box::new(item.clone())));
                            let spread =
                                self.infer(operand, environment, spread_expected.as_ref())?;
                            let resolved = self.resolve(&spread);
                            let TypeDescriptor::Dict(spread_item) = resolved else {
                                return Err(format!(
                                    "Dict spread requires Dict, found {}",
                                    resolved.display_name()
                                ));
                            };
                            item_types.push(*spread_item);
                        } else {
                            item_types.push(self.infer(
                                &field.value.value,
                                environment,
                                item_expected.as_ref(),
                            )?);
                        }
                    }
                    TypeDescriptor::Dict(Box::new(
                        item_expected.unwrap_or_else(|| join_all_types(item_types)),
                    ))
                } else {
                    if let Some(TypeDescriptor::Dict(item)) = expected.map(|ty| self.resolve(ty)) {
                        for field in fields {
                            self.infer(&field.value.value, environment, Some(&item))
                                .map_err(|message| {
                                    format!(
                                        "field {}: {message}",
                                        field
                                            .value
                                            .name
                                            .as_ref()
                                            .expect("ordinary Dict field has a name")
                                            .value
                                    )
                                })?;
                        }
                        TypeDescriptor::Dict(item)
                    } else {
                        let expected_fields = match expected.map(|ty| self.resolve(ty)) {
                            Some(TypeDescriptor::Struct(fields)) => fields,
                            _ => BTreeMap::new(),
                        };
                        TypeDescriptor::Struct(
                            fields
                                .iter()
                                .map(|field| {
                                    let name = field
                                        .value
                                        .name
                                        .as_ref()
                                        .expect("ordinary Dict field has a name")
                                        .value
                                        .clone();
                                    Ok((
                                        name.clone(),
                                        self.infer(
                                            &field.value.value,
                                            environment,
                                            expected_fields.get(&name),
                                        )
                                        .map_err(|message| format!("field {name}: {message}"))?,
                                    ))
                                })
                                .collect::<Result<_, String>>()?,
                        )
                    }
                }
            }
            ExprKind::Unary { operator, operand } => match operator.value {
                UnaryOperator::Negate => {
                    let numeric = self.fresh_variable();
                    self.require_numeric(&numeric)?;
                    if let Some(expected) = expected {
                        self.check(&numeric, expected)?;
                    }
                    let operand = self.infer(operand, environment, Some(&numeric))?;
                    self.require_numeric(&operand)?;
                    self.resolve(&numeric)
                }
                UnaryOperator::Not => {
                    let resolved_expected = expected.map(|expected| self.resolve(expected));
                    let expected_family =
                        resolved_expected
                            .as_ref()
                            .and_then(|expected| match expected {
                                TypeDescriptor::Int => Some(NotFamily::Int),
                                TypeDescriptor::Enum(variants)
                                    if TypeDescriptor::Enum(variants.clone())
                                        == normalized_bool_descriptor() =>
                                {
                                    Some(NotFamily::Bool)
                                }
                                TypeDescriptor::Any => Some(NotFamily::Dynamic),
                                _ => None,
                            });
                    let operand_expectation = resolved_expected.as_ref().filter(|expected| {
                        matches!(expected, TypeDescriptor::Int | TypeDescriptor::Any)
                            || matches!(
                                expected,
                                TypeDescriptor::Enum(variants)
                                    if TypeDescriptor::Enum(variants.clone())
                                        == normalized_bool_descriptor()
                            )
                    });
                    let operand = self.infer(operand, environment, operand_expectation)?;
                    self.require_not_operand(&operand)?;
                    let resolved_operand = self.resolve(&operand);
                    let family = expected_family.unwrap_or(match &resolved_operand {
                        TypeDescriptor::Int => NotFamily::Int,
                        TypeDescriptor::Atom(Atom::Builtin(
                            BuiltinAtom::True | BuiltinAtom::False,
                        ))
                        | TypeDescriptor::Enum(_) => NotFamily::Bool,
                        _ => NotFamily::Dynamic,
                    });
                    self.not_families.insert(expression.location, family);
                    let result = match family {
                        NotFamily::Bool => normalized_bool_descriptor(),
                        NotFamily::Int => TypeDescriptor::Int,
                        NotFamily::Dynamic => resolved_operand,
                    };
                    if let Some(expected) = expected {
                        self.check(&result, expected)?;
                    }
                    result
                }
                UnaryOperator::LogicalNot => {
                    let bool_type = normalized_bool_descriptor();
                    self.infer(operand, environment, Some(&bool_type))?;
                    bool_type
                }
                UnaryOperator::BitNot => {
                    self.infer(operand, environment, Some(&TypeDescriptor::Int))?;
                    TypeDescriptor::Int
                }
            },
            ExprKind::Propagate { operand } => {
                let operand = self.infer(operand, environment, None)?;
                match self.resolve(&operand) {
                    TypeDescriptor::Enum(variants) => {
                        if let Some(payload) = option_parts(&variants) {
                            self.propagation_families
                                .insert(expression.location, PropagationFamily::Option);
                            self.record_propagation(PropagationRequirement::Option)?;
                            payload.clone()
                        } else if let Some((ok, err)) =
                            result_parts(&TypeDescriptor::Enum(variants))
                        {
                            self.propagation_families
                                .insert(expression.location, PropagationFamily::Result);
                            let ok = ok.clone();
                            let err = err.clone();
                            self.record_propagation(PropagationRequirement::Result(vec![err]))?;
                            ok
                        } else {
                            return Err(
                                "? operand must be an exact Option-shaped or Result-shaped Enum"
                                    .into(),
                            );
                        }
                    }
                    descriptor => {
                        return Err(format!(
                            "? operand must resolve to an Option-shaped or Result-shaped Enum, found {}",
                            descriptor.display_name()
                        ));
                    }
                }
            }
            ExprKind::Return { value } => {
                let expected = self
                    .return_boundaries
                    .last()
                    .and_then(Option::as_ref)
                    .ok_or_else(|| "return is allowed only inside a Function".to_owned())?
                    .expected
                    .clone();
                let value = self.infer_authored_boundary(value, environment, expected.as_ref())?;
                self.return_boundaries
                    .last_mut()
                    .and_then(Option::as_mut)
                    .expect("Function return boundary exists")
                    .values
                    .push(value);
                TypeDescriptor::Never
            }
            ExprKind::Panic { message } => {
                self.infer(message, environment, Some(&TypeDescriptor::String))?;
                TypeDescriptor::Never
            }
            ExprKind::Raise { error } => {
                self.infer(error, environment, Some(&blame_error_descriptor()))?;
                TypeDescriptor::Never
            }
            ExprKind::Debug { value, .. } => self.infer(value, environment, expected)?,
            ExprKind::TypeAscription { value, target } => {
                self.infer(target, environment, Some(&TypeDescriptor::Type))?;
                let target = self
                    .local_annotations
                    .get(&target.location)
                    .cloned()
                    .ok_or_else(|| {
                        "type ascription target metadata was not evaluated".to_owned()
                    })?;
                let inferred = self.infer_authored_boundary(value, environment, Some(&target))?;
                self.check(&inferred, &target)?;
                target
            }
            ExprKind::CheckedCast { value, target } => {
                self.infer(target, environment, Some(&TypeDescriptor::Type))?;
                let target = self
                    .local_annotations
                    .get(&target.location)
                    .cloned()
                    .ok_or_else(|| "cast target metadata was not evaluated".to_owned())?;
                self.infer(value, environment, None)?;
                result_descriptor(target, TypeDescriptor::String)
            }
            ExprKind::DynProject {
                namespace,
                target,
                value,
            } => {
                let ExprKind::Variable(namespace_name) = &namespace.value else {
                    return Err("Dyn project syntax requires a std/dyn namespace".into());
                };
                if !self.dyn_namespaces.contains(&namespace_name.value) {
                    return Err(format!(
                        "{}.project@[T] is available only on an imported std/dyn namespace",
                        namespace_name.value
                    ));
                }
                self.infer(namespace, environment, None)?;
                let target_descriptor = self
                    .local_annotations
                    .get(&target.location)
                    .cloned()
                    .ok_or_else(|| "Dyn projection target metadata was not evaluated".to_owned())?;
                if type_identity_is_symbolic(&target_descriptor) {
                    return Err(
                        "Dyn projection of a generic type requires an explicit runtime TypeOf witness"
                            .into(),
                    );
                }
                self.infer(target, environment, Some(&TypeDescriptor::Type))?;
                self.infer(value, environment, Some(&TypeDescriptor::Dyn))?;
                option_descriptor(target_descriptor)
            }
            ExprKind::Binary {
                operator,
                left,
                right,
            } => match operator.value {
                BinaryOperator::And | BinaryOperator::Or => {
                    let bool_type = normalized_bool_descriptor();
                    self.infer(left, environment, Some(&bool_type))?;
                    self.infer(right, environment, Some(&bool_type))?;
                    bool_type
                }
                BinaryOperator::Equal | BinaryOperator::NotEqual => {
                    let left_constructs = expression_constructs_declared_value(left);
                    let right_constructs = expression_constructs_declared_value(right);
                    if left_constructs != right_constructs {
                        let (evidence, literal) = if left_constructs {
                            (right, left)
                        } else {
                            (left, right)
                        };
                        let evidence = self.infer(evidence, environment, None)?;
                        let evidence = self.resolve(&evidence);
                        if self.declared_identity(&evidence).is_some() {
                            self.infer(literal, environment, Some(&evidence))?;
                        } else {
                            let literal = self.infer(literal, environment, None)?;
                            self.unify_equality(&evidence, &literal)?;
                        }
                    } else {
                        let left = self.infer(left, environment, None)?;
                        let right = self.infer(right, environment, None)?;
                        self.unify_equality(&left, &right)?;
                    }
                    normalized_bool_descriptor()
                }
                BinaryOperator::BitAnd | BinaryOperator::BitOr | BinaryOperator::BitXor => {
                    if let Some(expected) = expected {
                        self.check(&TypeDescriptor::Int, expected)?;
                    }
                    self.infer(left, environment, Some(&TypeDescriptor::Int))?;
                    self.infer(right, environment, Some(&TypeDescriptor::Int))?;
                    TypeDescriptor::Int
                }
                BinaryOperator::LessThan
                | BinaryOperator::LessThanOrEqual
                | BinaryOperator::GreaterThan
                | BinaryOperator::GreaterThanOrEqual => {
                    let ordered = self.fresh_variable();
                    self.require_ordered(&ordered)?;
                    let left = self.infer(left, environment, Some(&ordered))?;
                    let right = self.infer(right, environment, Some(&ordered))?;
                    self.require_ordered(&left)?;
                    self.require_ordered(&right)?;
                    normalized_bool_descriptor()
                }
                _ => {
                    let numeric = self.fresh_variable();
                    self.require_numeric(&numeric)?;
                    if let Some(expected) = expected {
                        self.check(&numeric, expected)?;
                    }
                    let left = self.infer(left, environment, Some(&numeric))?;
                    let right = self.infer(right, environment, Some(&numeric))?;
                    self.require_numeric(&left)?;
                    self.require_numeric(&right)?;
                    self.resolve(&numeric)
                }
            },
            ExprKind::Field { receiver, field } => {
                if let ExprKind::Variable(module) = &receiver.value
                    && let Some(scheme) = self
                        .external_interfaces
                        .get(&module.value)
                        .and_then(|interface| interface.exports.get(&field.value))
                        .cloned()
                {
                    self.infer(receiver, environment, None)?;
                    self.instantiate(&scheme)
                } else {
                    let receiver = self.infer(receiver, environment, None)?;
                    self.project_field(&receiver, &field.value)?
                }
            }
            ExprKind::Index { receiver, index } => {
                let receiver = self.infer(receiver, environment, None)?;
                let receiver = self.expose_named(&receiver);
                let result = match receiver {
                    TypeDescriptor::Array(item) => *item,
                    TypeDescriptor::Inference(variable) => {
                        let item = self.fresh_variable();
                        self.bind_inference_variable(
                            variable,
                            &TypeDescriptor::Array(Box::new(item.clone())),
                        )?;
                        item
                    }
                    TypeDescriptor::Never => TypeDescriptor::Never,
                    TypeDescriptor::Any => TypeDescriptor::Any,
                    descriptor => {
                        return Err(format!(
                            "cannot index value of type {}",
                            descriptor.display_name()
                        ));
                    }
                };
                self.infer(index, environment, Some(&TypeDescriptor::Int))?;
                result
            }
            ExprKind::TupleProjection { receiver, index } => {
                let receiver = self.infer(receiver, environment, None)?;
                self.project_tuple(&receiver, index.value)?
            }
            ExprKind::Call { callee, arguments } => {
                let callee_has_runtime_boundary = self.callee_has_runtime_boundary(callee);
                if self.is_builtin_tuple(callee)
                    && let [argument] = arguments.as_slice()
                    && let ExprKind::Array(items) = &argument.value
                    && items
                        .iter()
                        .all(|item| !matches!(item.value, ExprKind::Spread(_)))
                {
                    self.infer(callee, environment, None)?;
                    let metadata_array = TypeDescriptor::Array(Box::new(TypeDescriptor::Type));
                    self.infer(argument, environment, Some(&metadata_array))?;
                    let mut tuple_items = Vec::with_capacity(items.len());
                    for item in items {
                        let item = self
                            .records
                            .get(&item.location)
                            .map(|item| self.resolve(item))
                            .ok_or_else(|| "Tuple item has no inferred Type metadata".to_owned())?;
                        match item {
                            TypeDescriptor::TypeOf(item) => tuple_items.push(*item),
                            TypeDescriptor::Type | TypeDescriptor::Any => {
                                tuple_items.push(TypeDescriptor::Any)
                            }
                            item => {
                                return Err(format!(
                                    "Tuple items must be Type metadata, found {}",
                                    item.display_name()
                                ));
                            }
                        };
                    }
                    let inferred =
                        TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Tuple(tuple_items)));
                    if let Some(expected) = expected {
                        self.check(&inferred, expected)?;
                    }
                    let inferred = self.resolve(&inferred);
                    self.records.insert(expression.location, inferred.clone());
                    return Ok(inferred);
                }
                let has_placeholder = matches!(
                    &callee.value,
                    ExprKind::TypeApply { arguments, .. }
                        if arguments
                            .iter()
                            .any(|argument| matches!(argument.value, TypeArgumentKind::Infer))
                );
                let callee = self.infer(callee, environment, None)?;
                let resolved_callee = self.resolve(&callee);
                let resolved_callee = if let TypeDescriptor::Inference(variable) = resolved_callee {
                    let function = TypeDescriptor::Function {
                        parameters: arguments.iter().map(|_| self.fresh_variable()).collect(),
                        result: Box::new(self.fresh_variable()),
                    };
                    self.bind_inference_variable(variable, &function)?;
                    function
                } else {
                    resolved_callee
                };
                match resolved_callee {
                    TypeDescriptor::Atom(tag) => {
                        if arguments.len() != 1 {
                            return Err(format!(
                                "tag constructor expects 1 argument, found {}",
                                arguments.len()
                            ));
                        }
                        let payload_expected = expected
                            .map(|expected| self.resolve(expected))
                            .and_then(|expected| match expected {
                                TypeDescriptor::Enum(variants) => {
                                    variants.get(tag.name()).and_then(|payload| payload.clone())
                                }
                                _ => None,
                            });
                        let payload =
                            self.infer(&arguments[0], environment, payload_expected.as_deref())?;
                        let result = TypeDescriptor::Tagged {
                            tag,
                            payload: Box::new(payload),
                        };
                        if let Some(expected) = expected {
                            self.check(&result, expected)?;
                        }
                        self.resolve(&result)
                    }
                    TypeDescriptor::Function { parameters, result } => {
                        if parameters.len() != arguments.len() {
                            return Err(format!(
                                "call expects {} arguments, found {}",
                                parameters.len(),
                                arguments.len()
                            ));
                        }
                        if let Some(expected) = expected {
                            self.check(&result, expected)?;
                        }
                        let mut partial_tagged_evidence = false;
                        let mut unresolved_argument_evidence = false;
                        let mut argument_order = (0..arguments.len()).collect::<Vec<_>>();
                        argument_order.sort_by_key(|index| match &arguments[*index].value {
                            _ if self.explicit_scheme(&arguments[*index]).is_some() => 0,
                            ExprKind::Dict(_) => 2,
                            ExprKind::Atom(_) => 3,
                            _ => 1,
                        });
                        for index in argument_order {
                            let argument = &arguments[index];
                            let parameter = &parameters[index];
                            let inference_expected = if contains_exposed_type_variable(parameter)
                                && matches!(argument.value, ExprKind::Variable(_))
                            {
                                None
                            } else {
                                Some(parameter)
                            };
                            let argument_type = if callee_has_runtime_boundary {
                                self.infer(argument, environment, inference_expected)?
                            } else {
                                self.infer_authored_boundary(
                                    argument,
                                    environment,
                                    inference_expected,
                                )?
                            };
                            unresolved_argument_evidence |=
                                contains_type_variable(&self.resolve(&argument_type));
                            partial_tagged_evidence |= matches!(
                                self.resolve(&argument_type),
                                TypeDescriptor::Tagged { .. }
                            );
                            if matches!(self.resolve(&argument_type), TypeDescriptor::Any) {
                                self.default_inference_variables_to_any(parameter);
                            }
                            if contains_exposed_type_variable(parameter) {
                                self.unify(&argument_type, parameter)?;
                            } else {
                                self.check(&argument_type, parameter)?;
                            }
                        }
                        self.materialize_field_requirements(&TypeDescriptor::Tuple(
                            parameters.clone(),
                        ))?;
                        if partial_tagged_evidence {
                            for parameter in &parameters {
                                self.default_inference_variables_to_any(parameter);
                            }
                        }
                        let result = self.resolve(&result);
                        let result = if matches!(result, TypeDescriptor::TypeOf(_))
                            && contains_type_variable(&result)
                        {
                            TypeDescriptor::Type
                        } else {
                            result
                        };
                        if contains_type_variable(&result)
                            && self.delayed_initializer_depth == 0
                            && !has_placeholder
                            && expected.is_none()
                            && !(self.closure_inference_depth > 0 && unresolved_argument_evidence)
                        {
                            return Err(format!(
                                "cannot infer generic result type {}",
                                result.display_name()
                            ));
                        }
                        result
                    }
                    TypeDescriptor::Any => {
                        for argument in arguments {
                            self.infer(argument, environment, None)?;
                        }
                        TypeDescriptor::Any
                    }
                    descriptor => {
                        for argument in arguments {
                            self.infer(argument, environment, None)?;
                        }
                        return Err(format!(
                            "cannot call value of type {}",
                            descriptor.display_name()
                        ));
                    }
                }
            }
            ExprKind::TypeApply { callee, arguments } => {
                let scheme = self.explicit_scheme(callee).ok_or_else(|| {
                    "explicit type application requires a statically known generic binding"
                        .to_owned()
                })?;
                if scheme.parameters.is_empty() {
                    return Err("cannot apply type arguments to a monomorphic binding".into());
                }
                if scheme.parameters.len() != arguments.len() {
                    return Err(format!(
                        "type application expects {} arguments, found {}",
                        scheme.parameters.len(),
                        arguments.len()
                    ));
                }
                self.infer(callee, environment, None)?;
                let type_expected = TypeDescriptor::Type;
                let mut replacements = HashMap::new();
                for (parameter, argument) in scheme.parameters.iter().zip(arguments) {
                    let descriptor = match &argument.value {
                        TypeArgumentKind::Explicit(expression) => {
                            self.infer(expression, environment, Some(&type_expected))?;
                            self.local_annotations
                                .get(&expression.location)
                                .cloned()
                                .ok_or_else(|| {
                                    "type argument metadata was not evaluated".to_owned()
                                })?
                        }
                        TypeArgumentKind::Infer => {
                            let descriptor = self.fresh_variable();
                            let TypeDescriptor::Inference(variable) = &descriptor else {
                                unreachable!("fresh variables are inference descriptors")
                            };
                            self.placeholder_obligations.push((
                                *variable,
                                argument.location,
                                parameter.name.clone(),
                            ));
                            self.records.insert(argument.location, descriptor.clone());
                            descriptor
                        }
                    };
                    replacements.insert(parameter.id, descriptor);
                }
                substitute_bound_parameters(&scheme.body, &replacements)
            }
            ExprKind::Interpreter { elaboration, .. } => {
                self.infer(elaboration, environment, expected)?
            }
            ExprKind::Closure {
                parameters,
                result_annotation,
                body,
            } => {
                let expected = match expected.map(|ty| self.resolve(ty)) {
                    Some(TypeDescriptor::Function {
                        parameters: expected_parameters,
                        result,
                    }) if expected_parameters.len() == parameters.len() => {
                        Some((expected_parameters, result))
                    }
                    _ => None,
                };
                let mut closure_environment = environment.clone();
                let mut parameter_types = Vec::with_capacity(parameters.len());
                for (index, parameter) in parameters.iter().enumerate() {
                    let surrounding = expected
                        .as_ref()
                        .and_then(|(parameters, _)| parameters.get(index));
                    let local = parameter.annotation.as_ref().and_then(|annotation| {
                        self.local_annotations.get(&annotation.location).cloned()
                    });
                    if local.as_ref().is_some_and(contains_any_descriptor) {
                        self.authored_any_definitions
                            .insert(parameter.name.location);
                    }
                    if let (Some(local), Some(surrounding)) = (&local, surrounding) {
                        self.check(local, surrounding)?;
                    }
                    let parameter_type = local
                        .or_else(|| surrounding.cloned())
                        .unwrap_or_else(|| self.fresh_variable());
                    if contains_any_descriptor(&parameter_type) {
                        self.authored_any_definitions
                            .insert(parameter.name.location);
                    }
                    parameter_types.push(parameter_type);
                }
                for (parameter, ty) in parameters.iter().zip(&parameter_types) {
                    closure_environment.insert(parameter.name.value.clone(), ty.clone());
                }
                let surrounding_result = (self.recursive_body_inference_depth == 0)
                    .then(|| expected.as_ref().map(|(_, result)| result.as_ref()))
                    .flatten();
                let local_result = result_annotation.as_ref().and_then(|annotation| {
                    self.local_annotations.get(&annotation.location).cloned()
                });
                if let (Some(local), Some(surrounding)) = (&local_result, surrounding_result) {
                    self.check(local, surrounding)?;
                }
                let result_expected = local_result.as_ref().or(surrounding_result);
                let inferring_unannotated = expected.is_none();
                if inferring_unannotated {
                    self.closure_inference_depth += 1;
                }
                self.scheme_scopes.push(
                    parameters
                        .iter()
                        .map(|parameter| (parameter.name.value.clone(), None))
                        .collect(),
                );
                self.propagation_boundaries.push(None);
                self.return_boundaries.push(Some(ReturnBoundary {
                    expected: result_expected.cloned(),
                    values: Vec::new(),
                }));
                let result = self.infer_block(body, &closure_environment, result_expected);
                self.scheme_scopes.pop();
                let return_boundary = self
                    .return_boundaries
                    .pop()
                    .and_then(|boundary| boundary)
                    .expect("closure return boundary exists");
                let requirement = self
                    .propagation_boundaries
                    .pop()
                    .expect("closure boundary exists");
                if inferring_unannotated {
                    self.closure_inference_depth -= 1;
                }
                let inferred_result =
                    self.finish_propagation_boundary(result?, result_expected, requirement)?;
                let inferred_result =
                    self.finish_return_boundary(inferred_result, return_boundary)?;
                let function = TypeDescriptor::Function {
                    parameters: parameter_types,
                    result: Box::new(local_result.unwrap_or(inferred_result)),
                };
                if expected.is_none()
                    && self.delayed_initializer_depth == 0
                    && contains_type_variable(&function)
                {
                    self.default_inference_variables_to_any(&function);
                }
                self.resolve(&function)
            }
            ExprKind::Block(block) => self.infer_block(block, environment, expected)?,
            ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let bool_type = normalized_bool_descriptor();
                self.infer(condition, environment, Some(&bool_type))?;
                let (then_type, else_type) = if let Some(expected) = expected
                    && contains_type_variable(&self.resolve(expected))
                {
                    let (then_expected, then_environment, then_evidence) =
                        self.freshen_join_context(expected, environment);
                    let then_type =
                        self.infer_block(then_branch, &then_environment, Some(&then_expected))?;
                    let (else_expected, else_environment, else_evidence) =
                        self.freshen_join_context(expected, environment);
                    let else_type =
                        self.infer_block(else_branch, &else_environment, Some(&else_expected))?;
                    self.merge_join_evidence(&[then_evidence, else_evidence])?;
                    (then_type, else_type)
                } else {
                    (
                        self.infer_block(then_branch, environment, expected)?,
                        self.infer_block(else_branch, environment, expected)?,
                    )
                };
                self.merge_structural_join_evidence(&[then_type.clone(), else_type.clone()])?;
                join_types(self.resolve(&then_type), self.resolve(&else_type))
            }
            ExprKind::IfLet {
                pattern,
                value,
                then_branch,
                else_branch,
            } => {
                let value_type = self.infer(value, environment, None)?;
                let resolved_value_type = self.expose_pattern_type(&value_type);
                let analysis = crate::pattern::analyze_pattern(pattern, &value_type);
                if analysis.compatibility == crate::pattern::PatternCompatibility::Incompatible
                    && analysis.problems.is_empty()
                {
                    let location =
                        crate::pattern::first_incompatible_location(pattern, &resolved_value_type)
                            .unwrap_or(pattern.location);
                    self.pattern_diagnostics.entry(location).or_insert_with(|| {
                        format!(
                            "pattern cannot match {}",
                            resolved_value_type.display_name()
                        )
                    });
                }
                for problem in analysis.problems {
                    self.pattern_diagnostics
                        .entry(problem.location)
                        .or_insert(problem.message);
                }
                let mut then_environment = environment.clone();
                self.scheme_scopes.push(HashMap::new());
                for binding in analysis.bindings {
                    self.pattern_binding_types
                        .insert(binding.location, binding.ty.clone());
                    self.set_local_scheme(binding.name.clone(), None);
                    then_environment.insert(binding.name, binding.ty);
                }
                let then_type = self.infer_block(then_branch, &then_environment, expected);
                self.scheme_scopes.pop();
                let then_type = then_type?;
                let else_type = self.infer_block(else_branch, environment, expected)?;
                self.merge_structural_join_evidence(&[then_type.clone(), else_type.clone()])?;
                join_types(self.resolve(&then_type), self.resolve(&else_type))
            }
            ExprKind::LetElse {
                pattern,
                value,
                else_branch,
                body,
            } => {
                let value_type = self.infer(value, environment, None)?;
                let resolved_value_type = self.expose_pattern_type(&value_type);
                let analysis = crate::pattern::analyze_pattern(pattern, &value_type);
                if analysis.irrefutable {
                    self.pattern_diagnostics
                        .entry(pattern.location)
                        .or_insert_with(|| "let else pattern is irrefutable".into());
                }
                if analysis.compatibility == crate::pattern::PatternCompatibility::Incompatible
                    && analysis.problems.is_empty()
                {
                    self.pattern_diagnostics
                        .entry(pattern.location)
                        .or_insert_with(|| {
                            format!(
                                "pattern cannot match {}",
                                resolved_value_type.display_name()
                            )
                        });
                }
                for problem in analysis.problems {
                    self.pattern_diagnostics
                        .entry(problem.location)
                        .or_insert(problem.message);
                }
                let else_type = self.infer_block(else_branch, environment, None)?;
                if !matches!(self.resolve(&else_type), TypeDescriptor::Never) {
                    return Err(format!(
                        "let else branch must have type Never, found {}",
                        self.resolve(&else_type).display_name()
                    ));
                }
                let mut body_environment = environment.clone();
                self.scheme_scopes.push(HashMap::new());
                for binding in analysis.bindings {
                    self.pattern_binding_types
                        .insert(binding.location, binding.ty.clone());
                    self.set_local_scheme(binding.name.clone(), None);
                    body_environment.insert(binding.name, binding.ty);
                }
                let body_type = self.infer_block(body, &body_environment, expected);
                self.scheme_scopes.pop();
                body_type?
            }
            ExprKind::Match { value, arms } => {
                let value_type = self.infer(value, environment, None)?;
                let resolved_value_type = self.expose_pattern_type(&value_type);
                let mut arm_types = Vec::with_capacity(arms.len());
                let mut arm_evidence = Vec::new();
                let mut covered_variants = BTreeSet::new();
                let mut all_values_covered = false;
                for arm in arms {
                    if let Some(query) = &self.query {
                        query.check().map_err(|error| error.to_string())?;
                    }
                    let (mut arm_environment, arm_expected, evidence) = if let Some(expected) =
                        expected
                        && contains_type_variable(&self.resolve(expected))
                    {
                        let (expected, environment, evidence) =
                            self.freshen_join_context(expected, environment);
                        (environment, Some(expected), Some(evidence))
                    } else {
                        (environment.clone(), None, None)
                    };
                    let analysis =
                        crate::pattern::analyze_pattern(&arm.value.pattern, &resolved_value_type);
                    if analysis.compatibility == crate::pattern::PatternCompatibility::Incompatible
                        && !arm.value.irrefutable_required
                        && analysis.problems.is_empty()
                    {
                        let location = crate::pattern::first_incompatible_location(
                            &arm.value.pattern,
                            &resolved_value_type,
                        )
                        .unwrap_or(arm.value.pattern.location);
                        self.pattern_diagnostics.entry(location).or_insert_with(|| {
                            format!(
                                "pattern cannot match {}",
                                resolved_value_type.display_name()
                            )
                        });
                    }
                    if arm.value.irrefutable_required && !analysis.irrefutable {
                        let location = crate::pattern::first_refutable_location(
                            &arm.value.pattern,
                            &resolved_value_type,
                        )
                        .unwrap_or(arm.value.pattern.location);
                        self.pattern_diagnostics.entry(location).or_insert_with(|| {
                            format!(
                                "refutable let pattern for {}",
                                resolved_value_type.display_name()
                            )
                        });
                    }
                    let redundant_variants = analysis
                        .possible_variants
                        .iter()
                        .filter(|variant| covered_variants.contains(*variant))
                        .cloned()
                        .collect::<Vec<_>>();
                    let unreachable = all_values_covered
                        || !analysis.possible_variants.is_empty()
                            && redundant_variants.len() == analysis.possible_variants.len();
                    if unreachable {
                        let message = if all_values_covered {
                            "unreachable match arm; prior arms cover every value".to_owned()
                        } else {
                            format!(
                                "unreachable match arm; prior arms cover {}",
                                redundant_variants
                                    .iter()
                                    .map(|variant| format!("'{variant}"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        self.pattern_diagnostics
                            .entry(arm.value.pattern.location)
                            .or_insert(message);
                    }
                    if arm.value.guard.is_none() {
                        covered_variants.extend(analysis.covered_variants.iter().cloned());
                        all_values_covered |= analysis.irrefutable;
                        if let TypeDescriptor::Enum(variants) = &resolved_value_type {
                            all_values_covered |= variants
                                .keys()
                                .all(|variant| covered_variants.contains(variant));
                        }
                    }
                    for problem in analysis.problems {
                        self.pattern_diagnostics
                            .entry(problem.location)
                            .or_insert(problem.message);
                    }
                    for duplicate in analysis.duplicates {
                        self.pattern_diagnostics
                            .entry(duplicate.location)
                            .or_insert_with(|| {
                                format!("duplicate pattern binding {:?}", duplicate.name)
                            });
                    }
                    self.scheme_scopes.push(HashMap::new());
                    for binding in analysis.bindings {
                        let binding_type = evidence
                            .as_ref()
                            .map(|replacements| {
                                replace_inference_variables(&binding.ty, replacements)
                            })
                            .unwrap_or(binding.ty);
                        self.pattern_binding_types
                            .insert(binding.location, binding_type.clone());
                        self.set_local_scheme(binding.name.clone(), None);
                        arm_environment.insert(binding.name, binding_type);
                    }
                    if let Some(guard) = &arm.value.guard {
                        self.infer(guard, &arm_environment, Some(&normalized_bool_descriptor()))?;
                    }
                    let arm_type = self.infer(
                        &arm.value.value,
                        &arm_environment,
                        arm_expected.as_ref().or(expected),
                    );
                    self.scheme_scopes.pop();
                    arm_types.push(arm_type?);
                    if let Some(evidence) = evidence {
                        arm_evidence.push(evidence);
                    }
                }
                self.merge_join_evidence(&arm_evidence)?;
                self.merge_structural_join_evidence(&arm_types)?;
                if let TypeDescriptor::Enum(variants) = &resolved_value_type {
                    let missing = variants
                        .iter()
                        .filter(|(name, _)| !covered_variants.contains(*name))
                        .map(|(name, payload)| {
                            if payload.is_some() {
                                format!("'{name}(_)")
                            } else {
                                format!("'{name}")
                            }
                        })
                        .collect::<Vec<_>>();
                    if !missing.is_empty() {
                        self.pattern_diagnostics
                            .entry(expression.location)
                            .or_insert_with(|| {
                                format!("non-exhaustive match; missing {}", missing.join(", "))
                            });
                    }
                }
                if let Some(first) = arm_types.first().cloned() {
                    arm_types
                        .into_iter()
                        .skip(1)
                        .fold(self.resolve(&first), |joined, arm| {
                            join_types(joined, self.resolve(&arm))
                        })
                } else {
                    TypeDescriptor::Any
                }
            }
        };
        if let Some(expected) = expected
            && !(self.recursive_body_inference_depth > 0
                && matches!(expression.value, ExprKind::Closure { .. }))
        {
            self.check(&inferred, expected)?;
        }
        let inferred = self.resolve(&inferred);
        self.records.insert(expression.location, inferred.clone());
        Ok(inferred)
    }

}

