impl<'a> GenericInference<'a> {
    fn infer_tuple_items(
        &mut self,
        items: &[Expr],
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&[TypeDescriptor]>,
    ) -> Result<Vec<TypeDescriptor>, String> {
        let mut types = Vec::new();
        for item in items {
            if let ExprKind::Spread(operand) = &item.value {
                let spread = if let ExprKind::Tuple(nested) = &operand.value {
                    let remaining = expected.map(|types_expected| {
                        &types_expected[types.len().min(types_expected.len())..]
                    });
                    let nested_types = self.infer_tuple_items(nested, environment, remaining)?;
                    self.records.insert(
                        operand.location,
                        TypeDescriptor::Tuple(nested_types.clone()),
                    );
                    nested_types
                } else {
                    let ty = self.infer(operand, environment, None)?;
                    let TypeDescriptor::Tuple(spread) = self.resolve(&ty) else {
                        return Err("tuple spread requires a statically known Tuple".into());
                    };
                    spread
                };
                for ty in spread {
                    if let Some(target) = expected.and_then(|expected| expected.get(types.len())) {
                        self.check(&ty, target)?;
                    }
                    types.push(ty);
                }
            } else {
                let target = expected.and_then(|expected| expected.get(types.len()));
                types.push(self.infer(item, environment, target)?);
            }
        }
        Ok(types)
    }

    fn infer_field_projection(
        &mut self,
        receiver: &Expr,
        fields: &[(crate::ast::Identifier, crate::ast::Identifier)],
        environment: &HashMap<String, TypeDescriptor>,
    ) -> Result<BTreeMap<String, TypeDescriptor>, String> {
        let source = self.infer(receiver, environment, None)?;
        let source = self
            .struct_update_fields(&source)
            .map_err(|_| "field projection requires a named struct source".to_string())?;
        let mut projected = BTreeMap::new();
        for (name, destination) in fields {
            let ty = source
                .get(&name.value)
                .ok_or_else(|| format!("unknown projection source field {:?}", name.value))?;
            if projected
                .insert(destination.value.clone(), ty.clone())
                .is_some()
            {
                return Err(format!("duplicate projection destination {:?}", destination.value));
            }
        }
        Ok(projected)
    }

    fn infer_struct_update(
        &mut self,
        expression: &Expr,
        environment: &HashMap<String, TypeDescriptor>,
        target: &BTreeMap<String, TypeDescriptor>,
    ) -> Result<(), String> {
        let mut contributed = BTreeMap::new();
        if let ExprKind::FieldProjection { receiver, fields } = &expression.value {
            contributed = self.infer_field_projection(receiver, fields, environment)?;
            self.records.insert(
                expression.location,
                TypeDescriptor::Struct(contributed.clone()),
            );
        } else if let ExprKind::Dict(entries) = &expression.value {
            let mut spreads = BTreeMap::new();
            let mut winners = BTreeMap::new();
            let mut explicit = BTreeSet::new();
            // Discover spread shapes before supplying context to winning literals.
            for (index, entry) in entries.iter().enumerate() {
                if let Some(name) = &entry.value.name {
                    if !explicit.insert(name.value.clone()) {
                        return Err(format!("duplicate update field {:?}", name.value));
                    }
                    winners.insert(name.value.clone(), index);
                } else if let ExprKind::Spread(operand) = &entry.value.value.value {
                    let ty = self.infer(operand, environment, None)?;
                    let fields = self.struct_update_fields(&ty)?;
                    for name in fields.keys() {
                        winners.insert(name.clone(), index);
                    }
                    spreads.insert(index, fields);
                }
            }
            for name in winners.keys() {
                if !target.contains_key(name) {
                    return Err(format!("unknown struct update field {name:?}"));
                }
            }
            for (index, entry) in entries.iter().enumerate() {
                if let Some(name) = &entry.value.name {
                    let expected =
                        (winners[&name.value] == index).then(|| &target[&name.value]);
                    let ty = self.infer(&entry.value.value, environment, expected)?;
                    contributed.insert(name.value.clone(), ty);
                } else if let Some(fields) = spreads.remove(&index) {
                    contributed.extend(fields);
                }
            }
            self.records.insert(
                expression.location,
                TypeDescriptor::Struct(contributed.clone()),
            );
        } else {
            let ty = self.infer(expression, environment, None)?;
            contributed = self.struct_update_fields(&ty)?;
        }
        for (name, ty) in contributed {
            let expected = target
                .get(&name)
                .ok_or_else(|| format!("unknown struct update field {name:?}"))?;
            self.check(&ty, expected)?;
        }
        Ok(())
    }

    fn struct_update_fields(
        &self,
        ty: &TypeDescriptor,
    ) -> Result<BTreeMap<String, TypeDescriptor>, String> {
        if let TypeDescriptor::Declared(declared) = self.expose_named(ty)
            && let TypeDescriptor::Struct(fields) = declared.body.as_ref()
        {
            return Ok(fields.clone());
        }
        Err("struct update requires a named struct operand".into())
    }

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
            .filter(|_| matches!(expression.value, ExprKind::Dict(_)))
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
                if let Some(expected) = expected.map(|ty| self.resolve(ty))
                    && ((constructs_declared_value
                        && matches!(expected, TypeDescriptor::Enum(_)))
                        || (matches!(expression.value, ExprKind::Atom(_))
                            && matches!(expected, TypeDescriptor::Function { .. })))
                {
                    self.records.insert(expression.location, expected.clone());
                    return expected;
                }
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



    fn infer_inner(
        &mut self,
        expression: &Expr,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        if let Some(query) = &self.query {
            query.check().map_err(|error| error.to_string())?;
        }
        self.value_constructors.remove(&expression.location);
        let inferred = match &expression.value {
            ExprKind::Variable(name) => match self.explicit_scheme(expression) {
                Some(scheme) => self.instantiate(&scheme, expression.location),
                None => environment.get(&name.value).cloned()
                    .ok_or_else(|| format!("unknown binding {:?}", name.value))?,
            },
            ExprKind::Int(_) => TypeDescriptor::Int,
            ExprKind::Float(_) => TypeDescriptor::Float,
            ExprKind::String(_) => TypeDescriptor::String,
            ExprKind::InterpolatedString(parts) => {
                for part in parts {
                    if let StringPartKind::Expression(expression) = &part.value {
                        let target = self.infer(expression, environment, None)?;
                        self.require_interpolation_evidence(target, expression.location)?;
                    }
                }
                TypeDescriptor::String
            }
            ExprKind::Bytes(_) => TypeDescriptor::Bytes,
            ExprKind::Atom(name) => self.enum_constructor(expression.location, name, None),
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
                    if item_types.iter().any(|ty| contains_type_variable(&self.resolve(ty))) {
                        // Empty spreads contribute an element variable, not an alternative
                        // to the concrete element evidence in the surrounding array.
                        let arrays = item_types.iter()
                            .map(|ty| TypeDescriptor::Array(Box::new(ty.clone())))
                            .collect::<Vec<_>>();
                        self.merge_structural_join_evidence(&arrays)?;
                    }
                    join_all_types(item_types.iter().map(|ty| self.resolve(ty)).collect())
                };
                TypeDescriptor::Array(Box::new(item))
            }
            ExprKind::Spread(operand) => self.infer(operand, environment, expected)?,
            ExprKind::Tuple(items) => {
                let item_expected = match expected.map(|ty| self.resolve(ty)) {
                    Some(TypeDescriptor::Tuple(expected_items)) => Some(expected_items),
                    _ => None,
                };
                TypeDescriptor::Tuple(self.infer_tuple_items(
                    items, environment, item_expected.as_deref(),
                )?)
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
                    let item = if let Some(expected) = item_expected {
                        expected
                    } else {
                        let dictionaries = item_types.iter()
                            .map(|ty| TypeDescriptor::Dict(Box::new(ty.clone())))
                            .collect::<Vec<_>>();
                        self.merge_structural_join_evidence(&dictionaries)?;
                        join_all_types(item_types.iter().map(|ty| self.resolve(ty)).collect())
                    };
                    TypeDescriptor::Dict(Box::new(item))
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
                                _ => None,
                            });
                    let operand_expectation = resolved_expected.as_ref().filter(|expected| {
                        matches!(expected, TypeDescriptor::Int)
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
                let value = self.infer(value, environment, expected.as_ref())?;
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
            ExprKind::Raise { action, message, subjects } => {
                let input = if matches!(action, crate::ast::BlameAction::Raise | crate::ast::BlameAction::Warn) {
                    TypeDescriptor::Opaque(crate::core::blame_native_type())
                } else { TypeDescriptor::String };
                self.infer(message, environment, Some(&input))?;
                for subject in subjects {
                    self.infer(subject, environment, None)?;
                }
                match action {
                    crate::ast::BlameAction::Build => TypeDescriptor::Opaque(crate::core::blame_native_type()),
                    crate::ast::BlameAction::Warn => option_descriptor(self.fresh_variable()),
                    _ => TypeDescriptor::Never,
                }
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
                let inferred = self.infer(value, environment, Some(&target))?;
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
                            self.infer(literal, environment, None)?;
                            let literal = self.contextualize_authored_literal(literal, &evidence)?;
                            self.unify_equality(&evidence, &literal)?;
                        }
                    } else {
                        self.infer(left, environment, None)?;
                        let right_type = self.infer(right, environment, None)?;
                        let left_type = self.contextualize_authored_literal(left, &right_type)?;
                        let right_type = self.contextualize_authored_literal(right, &left_type)?;
                        self.unify_equality(&left_type, &right_type)?;
                    }
                    normalized_bool_descriptor()
                }
                BinaryOperator::StructUpdate => {
                    let base = self.infer(left, environment, None)?;
                    let base = self.expose_named(&base);
                    if let TypeDescriptor::Declared(declared) = &base
                        && let TypeDescriptor::Struct(fields) = declared.body.as_ref()
                    {
                        self.infer_struct_update(right, environment, fields)?;
                        base
                    } else {
                        return Err("struct update requires a named struct base".into());
                    }
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
            ExprKind::FieldProjection { receiver, fields } => {
                let target = expected.map(|ty| self.expose_named(ty));
                let Some(TypeDescriptor::Declared(declared)) = target else {
                    return Err("field projection requires a named struct target context".into());
                };
                if !matches!(declared.body.as_ref(), TypeDescriptor::Struct(_)) {
                    return Err("field projection requires a named struct target context".into());
                }
                let projected = self.infer_field_projection(receiver, fields, environment)?;
                self.check(&TypeDescriptor::Struct(projected), &declared.body)?;
                TypeDescriptor::Declared(declared)
            }
            ExprKind::Field { receiver, field } => {
                if self.declared_constructor_reference(receiver) {
                    self.type_facet_locations.insert(receiver.location);
                    let receiver_type = self.infer(receiver, environment, None)?;
                    if let Some((ty, constructor)) = enum_member_type(&self.resolve(&receiver_type), &field.value)? {
                        self.value_constructors.insert(expression.location, constructor);
                        ty
                    } else {
                        self.project_field(&receiver_type, &field.value)?
                    }
                } else if let Some(scheme) = self
                        .namespace_interface(receiver)
                        .and_then(|interface| interface.exports.get(&field.value))
                        .cloned()
                {
                    self.infer(receiver, environment, None)?;
                    self.instantiate(&scheme, expression.location)
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
                if let ExprKind::Atom(tag) = &callee.value {
                    let [argument] = arguments.as_slice() else {
                        return Err(format!("tag constructor expects 1 argument, found {}", arguments.len()));
                    };
                    let target = expected.map(|ty| self.expose_named(ty));
                    let target_body = target.as_ref().map(|ty| match ty {
                        TypeDescriptor::Declared(declared) => declared.body.as_ref(),
                        ty => ty,
                    });
                    let payload_expected = target_body.and_then(|ty| match ty {
                        TypeDescriptor::Enum(variants) => variants.get(tag).and_then(Option::as_deref),
                        _ => None,
                    });
                    let payload = self.infer(argument, environment, payload_expected)?;
                    let owner = self.enum_constructor(expression.location, tag, Some((Some(argument.clone()), payload.clone())));
                    if let Some(expected) = expected {
                        self.check(&owner, expected)?;
                    }
                    self.records.insert(callee.location, TypeDescriptor::Function {
                        parameters: vec![payload], result: Box::new(owner.clone()),
                    });
                    self.records.insert(expression.location, owner.clone());
                    return Ok(self.resolve(&owner));
                }
                if let Some(result) =
                    self.infer_trait_call(callee, arguments, environment, expected)
                {
                    result?
                } else {
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
                    let mut has_complete_witnesses = true;
                    for item in items {
                        let item = self
                            .records
                            .get(&item.location)
                            .map(|item| self.resolve(item))
                            .ok_or_else(|| "Tuple item has no inferred Type metadata".to_owned())?;
                        match item {
                            TypeDescriptor::TypeOf(item) => tuple_items.push(*item),
                            TypeDescriptor::Type => has_complete_witnesses = false,
                            item => {
                                return Err(format!(
                                    "Tuple items must be Type metadata, found {}",
                                    item.display_name()
                                ));
                            }
                        };
                    }
                    let inferred = if has_complete_witnesses {
                        TypeDescriptor::TypeOf(Box::new(TypeDescriptor::Tuple(tuple_items)))
                    } else {
                        TypeDescriptor::Type
                    };
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
                let model_fields = if matches!(&callee.value, ExprKind::Variable(name)
                    if matches!(name.value.as_str(), "\0telora_enum" | "\0telora_struct" | "\0telora_newtype"))
                    && let Some(Expr { value: ExprKind::Dict(fields), .. }) = arguments.get(1)
                {
                    Some(TypeDescriptor::Struct(fields.iter().filter_map(|field| {
                        let name = field.value.name.as_ref()?;
                        let ty = if matches!(&field.value.value.value, ExprKind::Atom(tag) if tag == "None") {
                            option_descriptor(TypeDescriptor::Type)
                        } else {
                            TypeDescriptor::Type
                        };
                        Some((name.value.clone(), ty))
                    }).collect()))
                } else {
                    None
                };
                if expected.is_some_and(|ty| expects_type_value(&self.resolve(ty))) {
                    self.type_facet_locations.insert(callee.location);
                }
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
                        let mut unresolved_argument_evidence = false;
                        // Macro-generated arguments can share a source location, so retain
                        // each inference result rather than rereading the location map.
                        let mut argument_types = vec![TypeDescriptor::Never; arguments.len()];
                        let mut argument_order = (0..arguments.len()).collect::<Vec<_>>();
                        argument_order.sort_by_key(|index| match &arguments[*index].value {
                            _ if self.explicit_scheme(&arguments[*index]).is_some() => 0,
                            ExprKind::Dict(_) | ExprKind::FieldProjection { .. } => 2,
                            ExprKind::Atom(_) => 3,
                            _ => 1,
                        });
                        for index in argument_order {
                            let argument = &arguments[index];
                            let parameter = &parameters[index];
                            let inference_expected = if index == 1 && model_fields.is_some() {
                                model_fields.as_ref()
                            } else if contains_exposed_type_variable(parameter)
                                && matches!(argument.value, ExprKind::Variable(_))
                                && !expects_type_value(&self.resolve(parameter))
                            {
                                None
                            } else {
                                Some(parameter)
                            };
                            let argument_type = self.infer(argument, environment, inference_expected)?;
                            argument_types[index] = argument_type.clone();
                            unresolved_argument_evidence |=
                                contains_type_variable(&self.resolve(&argument_type));
                            if contains_exposed_type_variable(parameter) {
                                self.unify(&argument_type, parameter)?;
                            } else {
                                self.check(&argument_type, parameter)?;
                            }
                        }
                        // Keep the instantiated parameter variables: equal resolved types
                        // do not imply that two parameters share a generic constraint.
                        for ((argument, actual), parameter) in arguments
                            .iter()
                            .zip(&argument_types)
                            .zip(&parameters)
                        {
                            let actual = if matches!(
                                argument.value,
                                ExprKind::Array(_) | ExprKind::Tuple(_)
                            ) || expression_constructs_declared_value(argument)
                            {
                                self.contextualize_authored_literal(argument, actual)?
                            } else {
                                actual.clone()
                            };
                            self.refine_argument_nominal_context(parameter, &actual)?;
                        }
                        for (argument, parameter) in arguments.iter().zip(&parameters) {
                            self.contextualize_authored_literal(argument, parameter)?;
                        }
                        self.materialize_field_requirements(&TypeDescriptor::Tuple(
                            parameters.clone(),
                        ))?;
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
                let pending_start = self.pending_type_constraints.len();
                if self.type_facet_locations.contains(&expression.location)
                    || expected.is_some_and(|ty| expects_type_value(&self.resolve(ty)))
                {
                    self.type_facet_locations.insert(callee.location);
                }
                self.infer(callee, environment, None)?;
                self.pending_type_constraints.truncate(pending_start);
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
                for constraint in &scheme.constraints {
                    if let Some(target) = replacements.get(&constraint.parameter) {
                        let capability = match &constraint.capability {
                            TypeCapability::Trait { id, name } => TypeCapability::Trait {
                                id: *id,
                                name: name.clone(),
                            },
                            TypeCapability::Property(property) => TypeCapability::Property(
                                substitute_bound_parameters(property, &replacements),
                            ),
                        };
                        self.pending_type_constraints.push(PendingTypeConstraint {
                            capability,
                            target: target.clone(),
                            location: expression.location,
                            lexical_evidence: self.lexical_type_evidence.clone(),
                        });
                    }
                }
                if let Some(constructor) = self.value_constructors.get(&callee.location).cloned() {
                    self.value_constructors.insert(expression.location, constructor);
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
                let expected = match expected.map(|ty| match ty {
                    TypeDescriptor::Function { .. } => ty.clone(),
                    _ => self.resolve(ty),
                }) {
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
                    if let (Some(local), Some(surrounding)) = (&local, surrounding) {
                        self.check(local, surrounding)?;
                    }
                    let parameter_type = local
                        .or_else(|| surrounding.cloned())
                        .unwrap_or_else(|| self.fresh_variable());
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
                let canonical_pattern = self.infer_pattern_constructors(pattern, &value_type, environment)?;
                let pattern = &canonical_pattern;
                let resolved_value_type = self.expose_pattern_type(&value_type);
                let analysis = crate::pattern::analyze_pattern(pattern, &self.resolve(&value_type));
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
                    let binding_type = self.require_pattern_binding(&binding)?;
                    self.pattern_binding_types
                        .insert(binding.location, binding_type.clone());
                    self.set_local_scheme(binding.name.clone(), None);
                    then_environment.insert(binding.name, binding_type);
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
                let canonical_pattern = self.infer_pattern_constructors(pattern, &value_type, environment)?;
                let pattern = &canonical_pattern;
                let resolved_value_type = self.expose_pattern_type(&value_type);
                let analysis = crate::pattern::analyze_pattern(pattern, &self.resolve(&value_type));
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
                    let binding_type = self.require_pattern_binding(&binding)?;
                    self.pattern_binding_types
                        .insert(binding.location, binding_type.clone());
                    self.set_local_scheme(binding.name.clone(), None);
                    body_environment.insert(binding.name, binding_type);
                }
                let body_type = self.infer_block(body, &body_environment, expected);
                self.scheme_scopes.pop();
                body_type?
            }
            ExprKind::Match { value, arms } => {
                let value_type = self.infer(value, environment, None)?;
                let patterns = arms.iter().map(|arm|
                    self.infer_pattern_constructors(&arm.value.pattern, &value_type, environment)
                ).collect::<Result<Vec<_>, _>>()?;
                let resolved_value_type = self.expose_pattern_type(&value_type);
                // These parser-generated intrinsics have an Option contract; this
                // is not enum synthesis for user-authored match expressions.
                let intrinsic_option = arms.first().is_some_and(|arm| {
                    matches!(&arm.value.pattern.value,
                        crate::ast::PatternKind::Tagged { payload, .. }
                        if matches!(&payload.value, crate::ast::PatternKind::Binding(name)
                            if name.value.starts_with("$should_ok:")
                                || name.value.starts_with("$try_unwrap:")))
                });
                let intrinsic_expected = intrinsic_option.then(|| result_parts(&resolved_value_type))
                    .flatten().map(|(success, _)| option_descriptor(success.clone()));
                let expected = expected.or(intrinsic_expected.as_ref());
                let mut arm_types = Vec::with_capacity(arms.len());
                let mut arm_evidence = Vec::new();
                let mut covered_variants = BTreeSet::new();
                let mut all_values_covered = false;
                for (arm, pattern) in arms.iter().zip(&patterns) {
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
                        crate::pattern::analyze_pattern(pattern, &resolved_value_type);
                    if analysis.compatibility == crate::pattern::PatternCompatibility::Incompatible
                        && !arm.value.irrefutable_required
                        && analysis.problems.is_empty()
                    {
                        let location = crate::pattern::first_incompatible_location(
                            pattern,
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
                            pattern,
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
                                    .map(|variant| variant.clone())
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
                        let ty = self.require_pattern_binding(&binding)?;
                        let binding_type = evidence
                            .as_ref()
                            .map(|replacements| {
                                replace_inference_variables(&ty, replacements)
                            })
                            .unwrap_or(ty);
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
                                format!("{name}(_)")
                            } else {
                                name.clone()
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
                if let Some(expected) = expected.filter(|ty| !contains_type_variable(&self.resolve(ty))) {
                    self.resolve(expected)
                } else if let Some(first) = arm_types.first().cloned() {
                    arm_types
                        .into_iter()
                        .skip(1)
                        .fold(self.resolve(&first), |joined, arm| {
                            join_types(joined, self.resolve(&arm))
                        })
                } else {
                    TypeDescriptor::Never
                }
            }
        };
        if let Some(constructor) = self.member_constructor_reference(expression) {
            self.value_constructors.insert(expression.location, constructor);
        }
        let inferred = if matches!(expression.value, ExprKind::Variable(_) | ExprKind::Field { .. } | ExprKind::TypeApply { .. })
            && self.declared_constructor_reference(expression)
            && !self.type_facet_locations.contains(&expression.location)
            && !expected.is_some_and(|ty| expects_type_value(&self.resolve(ty)))
            && let Some(constructor) = newtype_constructor_type(&inferred)
        {
            self.value_constructors.insert(expression.location, ValueConstructor::Newtype);
            constructor
        } else {
            inferred
        };
        if let Some(expected) = expected
            && !(self.recursive_body_inference_depth > 0
                && matches!(expression.value, ExprKind::Closure { .. }))
        {
            self.check(&inferred, expected)?;
        }
        let inferred = match expected.map(|ty| self.resolve(ty)) {
            Some(expected) if contains_pending_alternatives(&inferred)
                && !contains_pending_alternatives(&expected)
                && !contains_type_variable(&expected) => expected,
            _ => self.resolve(&inferred),
        };
        self.records.insert(expression.location, inferred.clone());
        Ok(inferred)
    }

    // Both operands have been inferred. Only authored constructors receive context;
    // revisiting arbitrary expressions here could rebrand an existing anonymous value.
    fn contextualize_authored_literal(
        &mut self,
        expression: &Expr,
        expected: &TypeDescriptor,
    ) -> Result<TypeDescriptor, String> {
        let actual = self.resolve(&self.records[&expression.location]);
        if let TypeDescriptor::Inference(variable) = &actual
            && self.enum_constructors.contains_key(variable)
        {
            if !matches!(self.resolve(expected), TypeDescriptor::Inference(_)) {
                self.check(&actual, expected)?;
            }
            return Ok(self.resolve(&actual));
        }
        // A named member fixes the enum owner, but its authored payload can still
        // receive nominal context learned from the rest of the enclosing call.
        if let ExprKind::Call { callee, arguments } = &expression.value
            && let [argument] = arguments.as_slice()
            && let Some(ValueConstructor::EnumMember { tag, has_payload: true }) =
                self.value_constructors.get(&callee.location).cloned()
        {
            let expected = self.expose_named(expected);
            let same_owner = match (self.declared_identity(&actual), self.declared_identity(&expected)) {
                (Some(actual), Some(expected)) => actual.constructor() == expected.constructor(),
                (None, None) => true,
                _ => false,
            };
            if same_owner
                && let Some((TypeDescriptor::Function { parameters, .. }, _)) =
                    enum_member_type(&TypeDescriptor::TypeOf(Box::new(expected)), &tag)?
                && let Some(TypeDescriptor::Function { parameters: original, result }) =
                    self.records.get(&callee.location).cloned()
            {
                let payload = self.contextualize_authored_literal(argument, &parameters[0])?;
                self.refine_argument_nominal_context(&original[0], &payload)?;
                let result = self.resolve(&result);
                self.records.insert(expression.location, result.clone());
                return Ok(result);
            }
        }
        if self.declared_identity(&actual).is_some() {
            return Ok(actual);
        }
        let expected = self.expose_named(expected);
        if let TypeDescriptor::Declared(declared) = &expected {
            if !expression_constructs_declared_value(expression) {
                return Ok(actual);
            }
            let structural = self.contextualize_authored_literal(expression, &declared.body)?;
            self.check(&structural, &declared.body)?;
            self.records.insert(expression.location, expected.clone());
            return Ok(expected);
        }
        let contextualized = match (&expression.value, &expected) {
            (ExprKind::Array(items), TypeDescriptor::Array(item_type)) => {
                let mut types = Vec::new();
                for item in items {
                    let ty = if let ExprKind::Spread(operand) = &item.value {
                        let spread = self.contextualize_authored_literal(operand, &expected)?;
                        let TypeDescriptor::Array(ty) = spread else {
                            return Ok(actual);
                        };
                        *ty
                    } else {
                        self.contextualize_authored_literal(item, item_type)?
                    };
                    types.push(ty);
                }
                TypeDescriptor::Array(Box::new(if types.is_empty() {
                    item_type.as_ref().clone()
                } else {
                    join_all_types(types)
                }))
            }
            (ExprKind::Tuple(items), TypeDescriptor::Tuple(types)) => {
                let mut result = Vec::new();
                for item in items {
                    if let ExprKind::Spread(operand) = &item.value {
                        let TypeDescriptor::Tuple(spread) =
                            self.resolve(&self.records[&operand.location])
                        else {
                            return Ok(actual);
                        };
                        let Some(slice) = types.get(result.len()..result.len() + spread.len()) else {
                            return Ok(actual);
                        };
                        let contextual = self.contextualize_authored_literal(
                            operand,
                            &TypeDescriptor::Tuple(slice.to_vec()),
                        )?;
                        let TypeDescriptor::Tuple(spread) = contextual else {
                            return Ok(actual);
                        };
                        result.extend(spread);
                    } else {
                        let Some(ty) = types.get(result.len()) else {
                            return Ok(actual);
                        };
                        result.push(self.contextualize_authored_literal(item, ty)?);
                    }
                }
                if result.len() != types.len() {
                    return Ok(actual);
                }
                TypeDescriptor::Tuple(result)
            }
            (ExprKind::Dict(fields), TypeDescriptor::Struct(types)) => {
                let mut result = BTreeMap::new();
                for field in fields {
                    let Some(name) = &field.value.name else {
                        return Ok(actual);
                    };
                    let Some(ty) = types.get(&name.value) else {
                        return Ok(actual);
                    };
                    result.insert(
                        name.value.clone(),
                        self.contextualize_authored_literal(&field.value.value, ty)?,
                    );
                }
                TypeDescriptor::Struct(result)
            }
            (ExprKind::Dict(fields), TypeDescriptor::Dict(item_type)) => {
                let mut types = Vec::new();
                for field in fields {
                    let ty = if let ExprKind::Spread(operand) = &field.value.value.value {
                        let spread = self.contextualize_authored_literal(operand, &expected)?;
                        let TypeDescriptor::Dict(ty) = spread else {
                            return Ok(actual);
                        };
                        *ty
                    } else {
                        self.contextualize_authored_literal(&field.value.value, item_type)?
                    };
                    types.push(ty);
                }
                TypeDescriptor::Dict(Box::new(if types.is_empty() {
                    item_type.as_ref().clone()
                } else {
                    join_all_types(types)
                }))
            }
            (ExprKind::Call { callee, arguments }, _)
                if matches!(callee.value, ExprKind::Atom(_)) && arguments.len() == 1 =>
            {
                let TypeDescriptor::Tagged { tag, .. } = &actual else {
                    return Ok(actual);
                };
                let payload_type = match &expected {
                    TypeDescriptor::Tagged { tag: expected_tag, payload } if tag == expected_tag => {
                        Some(payload.as_ref())
                    }
                    TypeDescriptor::Enum(variants) => {
                        variants.get(tag.name()).and_then(|ty| ty.as_deref())
                    }
                    _ => None,
                };
                let Some(payload_type) = payload_type else {
                    return Ok(actual);
                };
                TypeDescriptor::Tagged {
                    tag: tag.clone(),
                    payload: Box::new(
                        self.contextualize_authored_literal(&arguments[0], payload_type)?,
                    ),
                }
            }
            _ => return Ok(actual),
        };
        self.records
            .insert(expression.location, contextualized.clone());
        Ok(contextualized)
    }
}
