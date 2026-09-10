impl<'a> GenericInference<'a> {
    fn new(
        schemes: &'a HashMap<String, TypeScheme>,
        hir: &'a HirProgram,
        external_interfaces: &'a BTreeMap<String, ModuleInterface>,
        named_types: &'a BTreeMap<String, TypeDescriptor>,
        annotation_inputs: InferenceAnnotationInputs,
        trait_implementations: &'a [TraitImplementation],
        type_properties: &'a [TypePropertyEvidence],
        trait_ids: &'a BTreeMap<String, crate::TraitId>,
        display_trait: Option<(crate::TraitId, String)>,
        dyn_namespaces: &'a HashSet<String>,
        builtin_tuple_available: bool,
        prepared_bodies: Option<&'a HashMap<crate::value::DeclaredTypeId, Arc<TypeDescriptor>>>,
        query: Option<crate::query::QueryContext>,
    ) -> Self {
        let mut declared_bodies = if let Some(bodies) = prepared_bodies {
            std::borrow::Cow::Borrowed(bodies)
        } else {
            let mut declared_bodies = HashMap::new();
            for scheme in schemes.values() {
                collect_declared_bodies(&scheme.body, &mut declared_bodies, &mut HashSet::new());
            }
            for descriptor in named_types.values() {
                collect_declared_bodies(descriptor, &mut declared_bodies, &mut HashSet::new());
            }
            for interface in external_interfaces.values() {
                for descriptor in interface.concrete_types.values() {
                    collect_declared_bodies(descriptor, &mut declared_bodies, &mut HashSet::new());
                }
                for scheme in interface.exports.values() {
                    collect_declared_bodies(&scheme.body, &mut declared_bodies, &mut HashSet::new());
                }
            }
            std::borrow::Cow::Owned(declared_bodies)
        };
        for (identity, slot) in &annotation_inputs.declared_bodies {
            if let Some(body) = annotation_inputs.variables.bound(*slot) {
                declared_bodies.to_mut().insert(identity.clone(), body);
            }
        }
        let mut variables = annotation_inputs.variables;
        let unresolved_reference_slots = hir.references().iter().map(|reference| {
            if !reference.resolution.is_unresolved() { return None; }
            let slot = variables.fresh();
            if let HirResolution::Conflicted(conflict) = reference.resolution {
                variables.record_conflict(&TypeDescriptor::Inference(slot),
                    &format!("conflicted symbol {:?}: {:?}", reference.name, hir.conflict(conflict)));
            }
            Some(slot)
        }).collect();
        Self {
            schemes,
            scheme_scopes: vec![HashMap::new()],
            top_level_inferred_schemes: HashMap::new(),
            inferred_schemes: HashMap::new(),
            inferred_runtime_scopes: HashMap::new(),
            placeholder_obligations: Vec::new(),
            pending_type_constraints: Vec::new(),
            trait_implementations,
            type_properties,
            local_type_properties: Vec::new(),
            property_contracts: HashMap::new(),
            trait_ids,
            display_trait,
            resolved_trait_members: HashMap::new(),
            resolved_call_evidence: HashMap::new(),
            resolved_interpolation_evidence: HashMap::new(),
            pending_interpolations: Vec::new(),
            runtime_type_evidence: BTreeMap::new(),
            lexical_type_evidence: Vec::new(),
            hir,
            external_interfaces,
            named_types,
            declared_bodies,
            local_annotations: annotation_inputs.types,
            normalizing_nominals: std::cell::RefCell::new(Vec::new()),
            dyn_namespaces,
            builtin_tuple_available,
            query,
            closure_inference_depth: 0,
            type_syntax_depth: 0,
            delayed_initializer_depth: 0,
            recursive_body_inference_depth: 0,
            numeric_variables: HashSet::new(),
            not_variables: HashSet::new(),
            ordered_variables: HashSet::new(),
            field_requirements: HashMap::new(),
            enum_constructors: HashMap::new(),
            value_constructors: HashMap::new(),
            type_facet_locations: HashSet::new(),
            recursive_equations: HashMap::new(),
            variables,
            unresolved_reference_slots,
            definition_bindings: vec![None; hir.definitions().len()],
            import_bindings: HashMap::new(),
            definition_schemes: Vec::new(),
            records: HashMap::new(),
            pattern_diagnostics: BTreeMap::new(),
            pattern_binding_types: HashMap::new(),
            propagation_boundaries: vec![None],
            return_boundaries: vec![None],
            propagation_families: HashMap::new(),
            not_families: HashMap::new(),
            failure_location: None,
            failure_expected_location: None,
            enum_failure: None,
            checking_named_pairs: HashSet::new(),
        }
    }

    fn record_type(&mut self, location: crate::Location, ty: TypeDescriptor) -> TypeDescriptor {
        // Contextual conversion replaces this expression's edge, not the source
        // variable's equality class.
        let slot = self.variables.structure_edge(ty);
        self.records.insert(location, slot);
        TypeDescriptor::Inference(slot)
    }

    fn take_failure_location(&mut self, fallback: crate::Location) -> crate::Location {
        self.failure_location.take().unwrap_or(fallback)
    }

    fn infer_pattern_constructors(
        &mut self,
        pattern: &crate::ast::Pattern,
        matched: &TypeDescriptor,
        environment: &dyn TypeEnvironment,
    ) -> Result<crate::ast::Pattern, String> {
        use crate::ast::PatternKind;
        if let PatternKind::Binding(name) = &pattern.value
            && self.hir.is_member_pattern(name.location)
        {
            return self.infer_pattern_constructors(&crate::ast::located(PatternKind::Constructor {
                constructor: Box::new(crate::ast::located(ExprKind::Variable(name.clone()), name.location)),
                payload: None,
            }, pattern.location), matched, environment);
        }
        if let PatternKind::Binding(name) = &pattern.value {
            self.pattern_binding_types.insert(name.location, matched.clone());
        }
        if let PatternKind::Constructor { constructor, payload } = &pattern.value {
            self.failure_location = Some(constructor.location);
            let ty = self.infer(constructor, environment, None)?;
            let kind = self.value_constructors.get(&constructor.location).cloned()
                .ok_or_else(|| "constructor pattern requires a type declaration".to_owned())?;
            let resolved = self.normalize(&ty);
            let has_payload = !matches!(kind, ValueConstructor::EnumMember { has_payload: false, .. });
            let (owner, payload_type) = if has_payload {
                let TypeDescriptor::Function { parameters, result } = resolved else {
                    return Err("constructor pattern requires a constructor function".into());
                };
                (result.as_ref().clone(), parameters.into_iter().next())
            } else {
                (resolved, None)
            };
            self.unify(matched, &owner)?;
            if payload.is_some() != has_payload {
                return Err(if has_payload {
                    "constructor pattern requires a payload pattern".into()
                } else {
                    "unit enum member does not accept a payload pattern".into()
                });
            }
            self.failure_location = None;
            let payload = match (payload, payload_type) {
                (Some(payload), Some(ty)) => Some(Box::new(self.infer_pattern_constructors(
                    payload, &self.normalize(&ty), environment,
                )?)),
                _ => None,
            };
            let value = match kind {
                ValueConstructor::Newtype => PatternKind::Constructor { constructor: constructor.clone(), payload },
                ValueConstructor::EnumMember { tag, .. } => match payload {
                    Some(payload) => PatternKind::Tagged { tag, payload },
                    None => PatternKind::Atom(tag),
                },
            };
            return Ok(crate::ast::located(value, pattern.location));
        }
        let mut canonical = pattern.clone();
        match (&mut canonical.value, self.expose_pattern_type(matched)) {
            (PatternKind::Tuple(items), TypeDescriptor::Tuple(types)) => {
                for (item, ty) in items.iter_mut().zip(types.iter()) {
                    *item = self.infer_pattern_constructors(item, ty, environment)?;
                }
            }
            (PatternKind::Struct(fields), TypeDescriptor::Struct(types)) => {
                for field in fields {
                    if let Some(ty) = types.get(&field.name.value) {
                        field.pattern = self.infer_pattern_constructors(&field.pattern, ty, environment)?;
                    }
                }
            }
            (PatternKind::Tagged { tag, payload }, TypeDescriptor::Enum(variants)) => {
                if let Some(Some(ty)) = variants.get(tag) {
                    **payload = self.infer_pattern_constructors(payload, ty, environment)?;
                }
            }
            (PatternKind::Tagged { payload, .. }, TypeDescriptor::Tagged { payload: ty, .. }) => {
                **payload = self.infer_pattern_constructors(payload, &ty, environment)?;
            }
            _ => {}
        }
        Ok(canonical)
    }

    fn require_pattern_binding(&mut self, binding: &crate::pattern::PatternBinding) -> Result<TypeDescriptor, String> {
        if let Some(ty) = &binding.ty {
            return Ok(ty.clone());
        }
        if let Some((location, message)) = self.pattern_diagnostics.first_key_value() {
            self.failure_location = Some(*location);
            return Err(message.clone());
        }
        // A constructor can establish a payload type even when the recursive
        // descriptor seen by the shape analyzer contains only a nominal stub.
        if let Some(ty) = self.pattern_binding_types.get(&binding.location) {
            let ty = self.normalize(ty);
            if !contains_type_variable(&ty) && !contains_pending_alternatives(&ty) {
                return Ok(ty);
            }
        }
        self.failure_location = Some(binding.location);
        Err(format!("cannot infer pattern binding {:?}; provide an explicit type context", binding.name))
    }

    fn take_failure_diagnostic(
        &mut self,
        fallback: crate::Location,
        message: String,
        expected_location: Option<crate::Location>,
    ) -> Diagnostic {
        let location = self.take_failure_location(fallback);
        let expected_location = self.failure_expected_location.take().or(expected_location);
        let Some(failure) = self.enum_failure.take() else {
            return Diagnostic::error(message, location);
        };
        let mut diagnostic = Diagnostic::error(message, location);
        if let Some(expected_location) = expected_location
            && expected_location != location
        {
            diagnostic = diagnostic.with_secondary(
                format!("expected type {} required here", failure.expected_name),
                expected_location,
            );
        }
        if failure.kind == EnumInferenceFailureKind::MissingContext {
            diagnostic = diagnostic.with_note(format!(
                "consider annotating the direct definition or collection as {}",
                failure.expected_name
            ));
        }
        diagnostic
    }

    fn record_failure_expected_location(&mut self, location: Option<crate::Location>) {
        if self.enum_failure.is_some() && self.failure_expected_location.is_none() {
            self.failure_expected_location = location;
        }
    }

    fn declared_body<'b>(&'b self, declared: &'b DeclaredTypeDescriptor) -> &'b TypeDescriptor {
        if matches!(declared.body.as_ref(), TypeDescriptor::Never) {
            self.declared_bodies
                .get(&declared.id)
                .map_or(declared.body.as_ref(), Arc::as_ref)
        } else {
            declared.body.as_ref()
        }
    }

    fn complete_declared(&self, descriptor: &TypeDescriptor) -> Option<TypeDescriptor> {
        let mut current = descriptor;
        let mut visited = HashSet::new();
        while let TypeDescriptor::Named(name) = current {
            if !visited.insert(name.clone()) {
                return None;
            }
            current = self.named_type(name)?;
        }
        let TypeDescriptor::Declared(declared) = current else {
            return None;
        };
        if declared.id.constructor() == unchecked_type_constructor() {
            return Some(self.normalize(current));
        }
        let body = if matches!(declared.body.as_ref(), TypeDescriptor::Never) {
            self.declared_bodies.get(&declared.id).unwrap_or(&declared.body)
        } else {
            &declared.body
        };
        Some(TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: declared.id.clone(),
            name: declared.name.clone(),
            body: Arc::clone(body),
        }))
    }

    fn expose_named(&self, ty: &TypeDescriptor) -> TypeDescriptor {
        let mut current = self.normalize(ty);
        let mut visited = HashSet::new();
        while let TypeDescriptor::Named(name) = &current {
            if !visited.insert(name.clone()) {
                break;
            }
            let Some(target) = self.named_type(name) else {
                break;
            };
            current = self.normalize(target);
        }
        if let Some(completed) = self.complete_declared(&current) {
            current = completed;
        }
        current
    }

    fn named_type(&self, name: &str) -> Option<&TypeDescriptor> {
        self.named_types.get(name).or_else(|| {
            let short = display_named_type(name);
            let mut candidates = self
                .named_types
                .iter()
                .filter(|(candidate, _)| display_named_type(candidate) == short)
                .map(|(_, descriptor)| descriptor);
            let candidate = candidates.next()?;
            let normalized = normalize_named_names(candidate);
            candidates
                .all(|other| normalize_named_names(other) == normalized)
                .then_some(candidate)
        })
    }

    fn named_identity(&self, ty: &TypeDescriptor) -> Option<String> {
        match ty {
            TypeDescriptor::Named(name) => Some(name.clone()),
            _ => None,
        }
    }

    fn declared_identity(&self, ty: &TypeDescriptor) -> Option<crate::value::DeclaredTypeId> {
        match self.nominal_view(ty)? {
            InferenceView::Row(row) => {
                let InferenceConstructor::Declared { head, .. } = self.variables.constructor(row) else { unreachable!() };
                if head.constructor() == unchecked_type_constructor() {
                    let TypeDescriptor::Declared(declared) = self.normalize(self.variables.descriptor_view(row)) else { unreachable!() };
                    return Some(declared.id);
                }
                let arguments = self.variables.arguments(row);
                let arguments = arguments[..arguments.len() - 1].iter().copied().map(TypeDescriptor::Inference).collect::<Vec<_>>();
                Some(head.reapply(&arguments))
            }
            InferenceView::Descriptor(descriptor @ TypeDescriptor::Declared(declared)) => {
                if declared.id.constructor() == unchecked_type_constructor() {
                    let TypeDescriptor::Declared(declared) = self.normalize(descriptor) else { unreachable!() };
                    Some(declared.id)
                } else { Some(declared.id.clone()) }
            }
            _ => unreachable!("nominal view"),
        }
    }

    fn finish_return_boundary(
        &mut self,
        tail: TypeDescriptor,
        boundary: ReturnBoundary,
    ) -> Result<TypeDescriptor, String> {
        if let Some(expected) = boundary.expected {
            for value in &boundary.values {
                self.check(value, &expected)?;
            }
            self.check(&tail, &expected)?;
            let mut values = boundary.values;
            values.push(tail);
            if let Some(actual) = common_type(values) {
                self.refine_argument_nominal_context(&expected, &actual)?;
            }
            return Ok(expected);
        }
        let mut values = boundary.values;
        values.push(tail);
        self.merge_structural_join_evidence(&values)?;
        Ok(join_all_types(values.iter().map(|value| self.normalize(value)).collect()))
    }

    fn record_propagation(&mut self, requirement: PropagationRequirement) -> Result<(), String> {
        let boundary = self
            .propagation_boundaries
            .last_mut()
            .expect("module propagation boundary exists");
        match (boundary.as_mut(), requirement) {
            (None, requirement) => *boundary = Some(requirement),
            (Some(PropagationRequirement::Option), PropagationRequirement::Option) => {}
            (
                Some(PropagationRequirement::Result(errors)),
                PropagationRequirement::Result(mut more),
            ) => {
                errors.append(&mut more);
            }
            _ => return Err("cannot mix Option and Result propagation in one boundary".into()),
        }
        Ok(())
    }

    fn finish_propagation_boundary(
        &mut self,
        result: TypeDescriptor,
        expected: Option<&TypeDescriptor>,
        requirement: Option<PropagationRequirement>,
    ) -> Result<TypeDescriptor, String> {
        let Some(requirement) = requirement else {
            return Ok(result);
        };
        let resolved = self.normalize(&result);
        match requirement {
            PropagationRequirement::Option => match resolved {
                TypeDescriptor::Inference(_) | TypeDescriptor::PendingAlternatives(_) => {
                    let success = self.fresh_variable();
                    let target = option_descriptor(success);
                    self.check(&resolved, &target)?;
                    Ok(self.normalize(&target))
                }
                TypeDescriptor::Enum(ref variants) if option_parts(variants).is_some() => {
                    Ok(resolved)
                }
                TypeDescriptor::Tagged { tag, payload } if tag.name() == "Some" => {
                    Ok(option_descriptor(*payload))
                }
                TypeDescriptor::Atom(tag) if tag.name() == "None" => {
                    match expected.map(|ty| self.normalize(ty)) {
                        Some(ref expected @ TypeDescriptor::Enum(ref variants)) if option_parts(variants).is_some() => Ok(expected.clone()),
                        _ => Err("Option propagation boundary ending in None needs an expected Option success type".into()),
                    }
                }
                _ => Err(format!(
                    "Option propagation requires an Option-shaped boundary result, found {}",
                    resolved.display_name()
                )),
            },
            PropagationRequirement::Result(errors) => {
                let expected = expected.map(|ty| self.normalize(ty));
                let boundary_error = expected
                    .as_ref()
                    .and_then(result_parts)
                    .map(|(_, err)| err.clone())
                    .or_else(|| common_type(errors.clone()))
                    .ok_or_else(|| "cannot infer Result propagation error type".to_owned())?;
                for error in &errors {
                    self.check(error, &boundary_error)?;
                }
                match resolved {
                    // A nonreturning tail does not erase earlier Err returns from `?`.
                    TypeDescriptor::Never => {
                        Ok(result_descriptor(TypeDescriptor::Never, boundary_error))
                    }
                    TypeDescriptor::Inference(_) | TypeDescriptor::PendingAlternatives(_) => {
                        let success = self.fresh_variable();
                        let target = result_descriptor(success, boundary_error);
                        self.check(&resolved, &target)?;
                        Ok(self.normalize(&target))
                    }
                    TypeDescriptor::Enum(ref variants) if result_parts(&TypeDescriptor::Enum(variants.clone())).is_some() => {
                        let (_, result_error) = result_parts(&resolved).expect("checked Result shape");
                        for error in &errors { self.check(error, result_error)?; }
                        Ok(resolved)
                    }
                    TypeDescriptor::Tagged { tag, payload } if tag.name() == "Ok" => {
                        Ok(result_descriptor(*payload, boundary_error))
                    }
                    TypeDescriptor::Tagged { tag, payload } if tag.name() == "Err" => {
                        match expected {
                            Some(ref expected @ TypeDescriptor::Enum(ref variants)) if result_parts(&TypeDescriptor::Enum(variants.clone())).is_some() => {
                                self.check(&payload, &boundary_error)?;
                                Ok(expected.clone())
                            }
                            _ => Err("Result propagation boundary ending in Err(_) needs an expected Result success type".into()),
                        }
                    }
                    _ => Err(format!("Result propagation requires a Result-shaped boundary result, found {}", resolved.display_name())),
                }
            }
        }
    }

    fn instantiate(&mut self, scheme: &TypeScheme, location: crate::Location) -> TypeDescriptor {
        let mut implicit_parameters = Vec::new();
        if scheme.parameters.is_empty() {
            collect_bound_parameters(&scheme.body, &mut implicit_parameters);
        }
        let parameters = scheme
            .parameters
            .iter()
            .map(|parameter| parameter.id)
            .chain(implicit_parameters);
        let mut variables: HashMap<TypeParameterId, InferenceVariableId> = parameters
            .map(|parameter| {
                let variable = self.variables.fresh();
                (parameter, variable)
            })
            .collect();
        for constraint in &scheme.constraints {
            if let Some(variable) = variables.get(&constraint.parameter).copied() {
                let capability = match &constraint.capability {
                    TypeCapability::RuntimeType => TypeCapability::RuntimeType,
                    TypeCapability::Trait { id, name } => TypeCapability::Trait {
                        id: *id,
                        name: name.clone(),
                    },
                    TypeCapability::Property(property) => TypeCapability::Property(
                        self.instantiate_with(property, &mut variables),
                    ),
                };
                self.pending_type_constraints.push(PendingTypeConstraint {
                    destination: EvidenceDestination::CallArgument,
                    capability,
                    target: TypeDescriptor::Inference(variable),
                    location,
                    lexical_evidence: self.lexical_type_evidence.clone(),
                });
            }
        }
        self.instantiate_with(&scheme.body, &mut variables)
    }

    fn scoped_scheme(&self, name: &str) -> Option<Option<TypeScheme>> {
        self.scheme_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn scheme(&self, name: &str) -> Option<TypeScheme> {
        match self.scoped_scheme(name) {
            Some(scheme) => scheme,
            None => self.schemes.get(name).cloned(),
        }
    }

    fn set_local_scheme(&mut self, name: String, scheme: Option<TypeScheme>) {
        self.scheme_scopes
            .last_mut()
            .expect("type inference always has a scheme scope")
            .insert(name, scheme);
    }

    fn resolved_binding(&self, name: &crate::ast::Identifier) -> Option<InferenceDefinition> {
        let reference = self.hir.reference_at(name.location, &name.value)?;
        if let Some(origin) = self.hir.reference_import_origin(reference.id) {
            return self.import_bindings.get(&origin).copied();
        }
        let HirResolution::Definition(id) = reference.resolution else { return None; };
        self.definition_bindings[id.index()]
    }

    fn bind_local(
        &mut self,
        environment: &mut dyn MutableTypeEnvironment,
        location: crate::Location,
        name: &str,
        descriptor: TypeDescriptor,
        scheme: Option<TypeScheme>,
    ) {
        let Some(definition) = self.hir.definition_at(location, name) else {
            environment.insert(name.to_owned(), descriptor);
            self.set_local_scheme(name.to_owned(), scheme);
            return;
        };
        let index = definition.id.index();
        if let Some(origin) = self.hir.definition_import_origin(definition.id) {
            let binding = self.bind_import_origin(origin, descriptor, scheme);
            self.definition_bindings[index] = Some(binding);
            return;
        }
        let slot = self.variables.structure_edge(descriptor);
        let scheme = scheme.map_or(u32::MAX, |scheme| {
            if let Some(previous) = self.definition_bindings[index]
                && previous.scheme != u32::MAX
            {
                self.definition_schemes[previous.scheme as usize] = scheme;
                previous.scheme
            } else {
                let id = u32::try_from(self.definition_schemes.len()).expect("inference scheme capacity exceeded");
                assert_ne!(id, u32::MAX, "inference scheme capacity exceeded");
                self.definition_schemes.push(scheme);
                id
            }
        });
        self.definition_bindings[index] = Some(InferenceDefinition { slot, scheme });
    }

    fn bind_import_origin(&mut self, origin: crate::hir::HirImportOrigin,
        descriptor: TypeDescriptor, scheme: Option<TypeScheme>) -> InferenceDefinition
    {
        if let Some(binding) = self.import_bindings.get(&origin) { return *binding; }
        let slot = self.variables.structure_edge(descriptor);
        let scheme = scheme.map_or(u32::MAX, |scheme| {
            let id = u32::try_from(self.definition_schemes.len()).expect("inference scheme capacity exceeded");
            assert_ne!(id, u32::MAX, "inference scheme capacity exceeded");
            self.definition_schemes.push(scheme);
            id
        });
        let binding = InferenceDefinition { slot, scheme };
        self.import_bindings.insert(origin, binding);
        binding
    }

    fn namespace_interface(&self, expression: &Expr) -> Option<&ModuleInterface> {
        match &expression.value {
            ExprKind::Variable(name) => self.external_interfaces.get(&name.value)
                .filter(|interface| interface.value_binding.is_none()),
            ExprKind::Field { receiver, field } => self.namespace_interface(receiver)?.namespaces.get(&field.value),
            _ => None,
        }
    }

    fn imported_expression_binding(&self, location: crate::Location) -> Option<InferenceDefinition> {
        self.hir.expression_import_origin_at(location)
            .and_then(|origin| self.import_bindings.get(&origin).copied())
    }

    fn instantiate_binding(&mut self, binding: InferenceDefinition, location: crate::Location) -> TypeDescriptor {
        if binding.scheme == u32::MAX { TypeDescriptor::Inference(binding.slot) }
        else { self.instantiate(&self.definition_schemes[binding.scheme as usize].clone(), location) }
    }

    fn explicit_scheme(&self, callee: &Expr) -> Option<TypeScheme> {
        if matches!(callee.value, ExprKind::Field { .. })
            && let Some(binding) = self.imported_expression_binding(callee.location)
        {
            return (binding.scheme != u32::MAX).then(|| self.definition_schemes[binding.scheme as usize].clone());
        }
        match &callee.value {
            ExprKind::Variable(name) => {
                if self.hir.reference_at(name.location, &name.value)
                    .is_some_and(|reference| reference.resolution.is_unresolved()) {
                    return None;
                }
                if let Some(binding) = self.resolved_binding(name) {
                    return (binding.scheme != u32::MAX).then(|| self.definition_schemes[binding.scheme as usize].clone());
                }
                self.member_import_definition(name)
                    .and_then(|definition| self.inferred_schemes.get(&definition.location).cloned()
                        .or_else(|| self.explicit_scheme(definition.member_import.as_ref()?)))
                    .or_else(|| self.scheme(&name.value))
            }
            ExprKind::Field { receiver, field } if self.declared_constructor_reference(receiver) => {
                let mut scheme = self.explicit_scheme(receiver)?;
                let (body, _) = enum_member_type(&scheme.body, &field.value).ok()??;
                scheme.body = body;
                if scheme.parameters.is_empty() {
                    let mut parameters = Vec::new();
                    collect_bound_parameters(&scheme.body, &mut parameters);
                    parameters.sort_unstable();
                    parameters.dedup();
                    scheme.parameters = parameters.into_iter().map(|id| TypeParameter {
                        id, name: format!("T{}", id.0), location: receiver.location,
                    }).collect();
                }
                Some(scheme)
            }
            ExprKind::Field { receiver, field } => self.namespace_interface(receiver)
                .and_then(|interface| interface.exports.get(&field.value)).cloned(),
            _ => None,
        }
    }

    fn member_import_definition(&self, name: &crate::ast::Identifier) -> Option<&crate::hir::HirDefinition> {
        let reference = self.hir.reference_at(name.location, &name.value)?;
        let HirResolution::Definition(id) = reference.resolution else { return None; };
        self.hir.definition(id).filter(|definition| definition.member_import.is_some())
    }

    fn member_constructor_reference(&self, expression: &Expr) -> Option<ValueConstructor> {
        match &expression.value {
            ExprKind::Variable(name) => {
                if let Some(import) = self.member_import_definition(name).and_then(|definition| definition.member_import.as_ref()) {
                    return self.value_constructors.get(&import.location).cloned()
                        .or_else(|| self.member_constructor_reference(import));
                }
                if self.hir.reference_at(name.location, &name.value).is_some_and(|reference|
                    matches!(reference.resolution, HirResolution::Definition(id)
                            if self.hir.definition(id).is_some_and(|definition| definition.kind != HirDefinitionKind::Import)))
                {
                    return None;
                }
                self.external_interfaces.get(&name.value)
                    .filter(|interface| interface.value_binding.as_deref() == Some(name.value.as_str()))
                    .and_then(|interface| interface.member_constructors.get(&name.value)).cloned()
            }
            ExprKind::Field { receiver, field } if self.declared_constructor_reference(receiver) => {
                let scheme = self.explicit_scheme(receiver)?;
                enum_member_type(&scheme.body, &field.value).ok().flatten().map(|(_, constructor)| constructor)
            }
            ExprKind::Field { receiver, field } => self.namespace_interface(receiver)
                .and_then(|interface| interface.member_constructors.get(&field.value)).cloned(),
            ExprKind::TypeApply { callee, .. } => self.member_constructor_reference(callee),
            _ => None,
        }
    }

    fn member_import_scheme(&mut self, binding: &crate::ast::BindingData, inferred: &TypeDescriptor) -> Result<TypeScheme, String> {
        if !matches!(&binding.value.value, ExprKind::Field { receiver, .. }
            if self.declared_constructor_reference(receiver))
            || self.member_constructor_reference(&binding.value).is_none()
        {
            return Err("member import requires an enum declaration member".into());
        }
        let scheme = self.explicit_scheme(&binding.value)
            .ok_or_else(|| "member import has no declaration contract".to_owned())?;
        self.unify(inferred, &scheme.body)?;
        Ok(scheme)
    }

    fn declared_constructor_reference(&self, expression: &Expr) -> bool {
        match &expression.value {
            ExprKind::Variable(name) => {
                if matches!(name.value.as_str(), "Bool" | "Option" | "Result" | "FoldControl" | "PropertyTarget")
                    && !self.external_interfaces.contains_key(&name.value)
                    && self.hir.reference_at(name.location, &name.value).is_some_and(|reference|
                        reference.resolution == HirResolution::External)
                {
                    return true;
                }
                if let Some(reference) = self.hir.reference_at(name.location, &name.value)
                    && let HirResolution::Definition(id) = reference.resolution
                    && let Some(definition) = self.hir.definition(id)
                    && definition.kind != HirDefinitionKind::Import
                {
                    return definition.kind == HirDefinitionKind::Type;
                }
                self.external_interfaces.get(&name.value)
                    .is_some_and(|interface| interface.value_binding.as_deref() == Some(name.value.as_str())
                        && interface.type_declarations.contains(&name.value))
            }
            ExprKind::Field { receiver, field } => self.namespace_interface(receiver)
                .is_some_and(|interface| interface.type_declarations.contains(&field.value)),
            ExprKind::TypeApply { callee, .. } => self.declared_constructor_reference(callee),
            _ => false,
        }
    }

    fn fresh_variable(&mut self) -> TypeDescriptor {
        let variable = self.variables.fresh();
        TypeDescriptor::Inference(variable)
    }

    fn merge_structural_join_evidence(
        &mut self,
        branches: &[TypeDescriptor],
    ) -> Result<(), String> {
        fn collect(
            unresolved: &TypeDescriptor,
            evidence: &TypeDescriptor,
            collected: &mut HashMap<InferenceVariableId, Vec<TypeDescriptor>>,
            may_infer: bool,
            enum_owners: &HashSet<InferenceVariableId>,
        ) {
            if let TypeDescriptor::Inference(variable) = unresolved {
                if (may_infer || enum_owners.contains(variable))
                    && !contains_type_variable(evidence) {
                    collected
                        .entry(*variable)
                        .or_default()
                        .push(evidence.clone());
                }
                return;
            }
            match (unresolved, evidence) {
                (TypeDescriptor::Array(left), TypeDescriptor::Array(right))
                | (TypeDescriptor::Dict(left), TypeDescriptor::Dict(right)) => {
                    collect(left, right, collected, true, enum_owners);
                }
                (TypeDescriptor::TypeOf(left), TypeDescriptor::TypeOf(right))
                | (TypeDescriptor::Newtype(left), TypeDescriptor::Newtype(right)) => {
                    collect(left, right, collected, may_infer, enum_owners);
                }
                (
                    TypeDescriptor::Tagged {
                        tag: left_tag,
                        payload: left,
                    },
                    TypeDescriptor::Tagged {
                        tag: right_tag,
                        payload: right,
                    },
                ) if left_tag == right_tag => {
                    collect(left, right, collected, may_infer, enum_owners);
                }
                (TypeDescriptor::Tuple(left), TypeDescriptor::Tuple(right))
                    if left.len() == right.len() =>
                {
                    for (left, right) in left.iter().zip(right) {
                        collect(left, right, collected, may_infer, enum_owners);
                    }
                }
                (TypeDescriptor::Struct(left), TypeDescriptor::Struct(right))
                    if left.keys().eq(right.keys()) =>
                {
                    for (name, left) in left {
                        collect(left, &right[name], collected, may_infer, enum_owners);
                    }
                }
                (TypeDescriptor::Enum(left), TypeDescriptor::Enum(right))
                    if left.keys().eq(right.keys()) =>
                {
                    for (name, left) in left {
                        if let (Some(left), Some(right)) = (left.as_deref(), right[name].as_deref())
                        {
                            collect(left, right, collected, true, enum_owners);
                        }
                    }
                }
                (TypeDescriptor::Declared(left), TypeDescriptor::Declared(right))
                    if left.id.has_same_head(&right.id)
                        && left.id.arguments().len() == right.id.arguments().len() =>
                {
                    for (left, right) in left.id.arguments().iter().zip(right.id.arguments()) {
                        collect(left, right, collected, true, enum_owners);
                    }
                }
                (
                    TypeDescriptor::Function {
                        parameters: left_parameters,
                        result: left_result,
                    },
                    TypeDescriptor::Function {
                        parameters: right_parameters,
                        result: right_result,
                    },
                ) if left_parameters.len() == right_parameters.len() => {
                    for (left, right) in left_parameters.iter().zip(right_parameters) {
                        collect(left, right, collected, may_infer, enum_owners);
                    }
                    collect(left_result, right_result, collected, may_infer, enum_owners);
                }
                _ => {}
            }
        }

        loop {
            let resolved = branches
                .iter()
                .map(|branch| self.normalize(branch))
                .collect::<Vec<_>>();
            let mut collected = HashMap::new();
            let enum_owners = self.enum_constructors.keys().copied().collect();
            for (index, branch) in resolved.iter().enumerate() {
                for evidence in resolved.iter().skip(index + 1) {
                    collect(branch, evidence, &mut collected, false, &enum_owners);
                    collect(evidence, branch, &mut collected, false, &enum_owners);
                }
            }
            if collected.is_empty() {
                break;
            }
            // Each pass solves previously unknown variables with concrete evidence;
            // a completed enum can then supply an empty collection's element type.
            for (variable, evidence) in collected {
                self.check(
                    &join_all_types(evidence),
                    &TypeDescriptor::Inference(variable),
                )?;
            }
        }
        Ok(())
    }

    fn generalize_local_closure(
        &mut self,
        descriptor: &TypeDescriptor,
        first_owned_variable: u32,
        location: crate::Location,
        expression_location: crate::Location,
    ) -> Result<Option<TypeScheme>, String> {
        let descriptor = self.normalize(descriptor);
        let mut variables = Vec::new();
        collect_inference_variables(&descriptor, &mut variables);
        variables.retain(|variable| variable.0 >= first_owned_variable);
        variables.dedup();
        if variables
            .iter()
            .any(|variable| self.field_requirements.contains_key(variable)
                || self.enum_constructors.contains_key(variable))
        {
            return Ok(None);
        }
        if variables.is_empty()
            || variables.iter().any(|variable| {
                self.numeric_variables.contains(variable)
                    || self.not_variables.contains(variable)
                    || self.ordered_variables.contains(variable)
            })
        {
            return Ok(None);
        }
        let mut bound_parameters = Vec::new();
        collect_bound_parameters(&descriptor, &mut bound_parameters);
        for evidence in self.lexical_type_evidence.iter().chain(self.inferred_runtime_scopes.values().flatten()) {
            collect_bound_parameters(&evidence.target, &mut bound_parameters);
        }
        let first_parameter = bound_parameters
            .iter()
            .map(|parameter| parameter.0)
            .max()
            .map_or(Some(0), |parameter| parameter.checked_add(1))
            .ok_or_else(|| "inferred type parameter identity overflow".to_owned())?;
        let replacements = variables
            .iter()
            .enumerate()
            .map(|(index, variable)| (*variable, TypeParameterId(first_parameter + index as u32)))
            .collect::<HashMap<_, _>>();
        let parameters: Vec<TypeParameter> = variables
            .iter()
            .enumerate()
            .map(|(index, _)| TypeParameter {
                id: TypeParameterId(first_parameter + index as u32),
                name: inferred_type_parameter_name(index),
                location,
            })
            .collect();
        for (variable, parameter) in &replacements {
            self.variables.set(*variable, TypeDescriptor::Bound(*parameter));
        }
        let name = format!("inferred:{}:{}", expression_location.start, expression_location.end);
        let witnesses = parameters.iter().enumerate().map(|(index, parameter)| LexicalTypeEvidence {
            capability: TypeCapability::RuntimeType,
            target: TypeDescriptor::Bound(parameter.id),
            name: evidence_parameter_name(&name, index),
        }).collect::<Vec<_>>();
        for constraint in &mut self.pending_type_constraints {
            if constraint.location.source == expression_location.source
                && expression_location.start <= constraint.location.start
                && constraint.location.end <= expression_location.end
            {
                constraint.target = bind_inference_variables(&constraint.target, &replacements);
                constraint.lexical_evidence.extend(witnesses.clone());
            }
        }
        self.inferred_runtime_scopes.insert(expression_location, witnesses);
        let constraints = parameters.iter().map(|parameter| TypeConstraint {
            parameter: parameter.id, capability: TypeCapability::RuntimeType, location,
        }).collect();
        Ok(Some(TypeScheme {
            parameters,
            constraints,
            body: bind_inference_variables(&descriptor, &replacements),
        }))
    }

    fn unresolved_placeholder_since(&self, start: usize) -> Option<(crate::Location, String)> {
        self.placeholder_obligations[start..]
            .iter()
            .find_map(|(variable, location, parameter)| {
                contains_type_variable(&self.normalize(&TypeDescriptor::Inference(*variable))).then(
                    || {
                        (
                            *location,
                            format!("cannot infer type argument `_` for parameter {parameter:?}"),
                        )
                    },
                )
            })
    }

    fn recursive_closure_skeleton(&mut self, expression: &Expr) -> Option<TypeDescriptor> {
        let ExprKind::Closure {
            parameters,
            result_annotation,
            ..
        } = &expression.value
        else {
            return None;
        };
        let parameters = parameters
            .iter()
            .map(|parameter| {
                parameter
                    .annotation
                    .as_ref()
                    .and_then(|annotation| self.local_annotations.get(&annotation.location))
                    .cloned()
                    .unwrap_or_else(|| self.fresh_variable())
            })
            .collect();
        let result = result_annotation
            .as_ref()
            .and_then(|annotation| self.local_annotations.get(&annotation.location))
            .cloned()
            .unwrap_or_else(|| self.fresh_variable());
        Some(TypeDescriptor::Function {
            parameters,
            result: Box::new(result),
        })
    }

    fn recursive_result_variable(descriptor: &TypeDescriptor) -> Option<InferenceVariableId> {
        match descriptor {
            TypeDescriptor::Function { result, .. } => match result.as_ref() {
                TypeDescriptor::Inference(variable) => Some(*variable),
                _ => None,
            },
            _ => None,
        }
    }

    fn recursive_approximation(
        &self,
        descriptor: &TypeDescriptor,
        variables: &HashSet<InferenceVariableId>,
        approximations: &HashMap<InferenceVariableId, TypeDescriptor>,
    ) -> Option<TypeDescriptor> {
        let head = self.variables.head(descriptor);
        let descriptor = &*head;
        match descriptor {
            TypeDescriptor::Inference(variable) if variables.contains(variable) => {
                approximations.get(variable).cloned()
            }
            TypeDescriptor::PendingAlternatives(variants) => {
                let resolved = variants
                    .iter()
                    .filter_map(|variant| {
                        self.recursive_approximation(variant, variables, approximations)
                    })
                    .collect::<Vec<_>>();
                (!resolved.is_empty()).then(|| pending_alternatives(resolved))
            }
            descriptor => {
                let resolved = self.normalize(descriptor);
                (!contains_any_inference_variable(&resolved, variables)).then_some(resolved)
            }
        }
    }

    fn solve_recursive_equations(
        &mut self,
        variables: &HashSet<InferenceVariableId>,
    ) -> Result<(), String> {
        let mut approximations = HashMap::new();
        for _ in 0..=variables.len() {
            let mut changed = false;
            let mut next = approximations.clone();
            for variable in variables {
                let Some(equation) = self.recursive_equations.get(variable) else {
                    continue;
                };
                if let Some(value) =
                    self.recursive_approximation(equation, variables, &approximations)
                    && next.get(variable) != Some(&value)
                {
                    next.insert(*variable, value);
                    changed = true;
                }
            }
            approximations = next;
            if !changed {
                break;
            }
        }
        for (variable, approximation) in approximations {
            self.bind_inference_variable(variable, &approximation)?;
        }
        Ok(())
    }

    fn instantiate_with(
        &mut self,
        ty: &TypeDescriptor,
        variables: &mut HashMap<TypeParameterId, InferenceVariableId>,
    ) -> TypeDescriptor {
        TypeDescriptor::Inference(self.variables.instantiate_descriptor(ty, variables))
    }

    fn normalize(&self, ty: &TypeDescriptor) -> TypeDescriptor {
        #[cfg(feature = "inference-profile")]
        let _profile = self.variables.profile.normalization();
        match ty {
            TypeDescriptor::Inference(variable) => self.variables.binding(*variable)
                .map_or_else(|| TypeDescriptor::Inference(self.variables.root(*variable)), |ty| self.normalize(ty)),
            TypeDescriptor::Declared(declared) => {
                let arguments = declared
                    .id
                    .arguments()
                    .iter()
                    .map(|argument| self.normalize(argument))
                    .collect::<Vec<_>>();
                if declared.id.constructor() == unchecked_type_constructor() {
                    return unchecked_descriptor(arguments[0].clone());
                }
                let identity = declared.id.reapply(&arguments);
                if self.normalizing_nominals.borrow().contains(&identity) {
                    return TypeDescriptor::Declared(DeclaredTypeDescriptor {
                        id: identity, name: declared.name.clone(), body: Arc::new(TypeDescriptor::Never),
                    });
                }
                self.normalizing_nominals.borrow_mut().push(identity.clone());
                let body = self.normalize_body(&declared.body);
                self.normalizing_nominals.borrow_mut().pop();
                TypeDescriptor::Declared(DeclaredTypeDescriptor {
                    id: identity,
                    name: declared.name.clone(),
                    body,
                })
            }
            TypeDescriptor::Array(item) => TypeDescriptor::Array(Box::new(self.normalize(item))),
            TypeDescriptor::Newtype(item) => TypeDescriptor::Newtype(Box::new(self.normalize(item))),
            TypeDescriptor::Dict(item) => TypeDescriptor::Dict(Box::new(self.normalize(item))),
            TypeDescriptor::TypeOf(instance) => {
                TypeDescriptor::TypeOf(Box::new(self.normalize(instance)))
            }
            TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
                tag: tag.clone(),
                payload: Box::new(self.normalize(payload)),
            },
            TypeDescriptor::Tuple(items) => {
                TypeDescriptor::Tuple(items.iter().map(|item| self.normalize(item)).collect())
            }
            TypeDescriptor::Struct(fields) => {
                TypeDescriptor::Struct(fields.iter()
                    .map(|(name, ty)| (name.clone(), self.normalize(ty))).collect())
            }
            TypeDescriptor::Enum(variants) => {
                TypeDescriptor::Enum(variants.iter().map(|(name, payload)| {
                    (name.clone(), payload.as_ref().map(|ty| Box::new(self.normalize(ty))))
                }).collect())
            }
            TypeDescriptor::PendingAlternatives(variants) => {
                let variants = variants
                    .iter()
                    .map(|variant| self.normalize(variant))
                    .collect::<Vec<_>>();
                pending_alternatives(variants)
            }
            TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
                parameters: parameters
                    .iter()
                    .map(|parameter| self.normalize(parameter))
                    .collect(),
                result: Box::new(self.normalize(result)),
            },
            ty => ty.clone(),
        }
    }

    fn normalize_body(&self, body: &Arc<TypeDescriptor>) -> Arc<TypeDescriptor> {
        let id = self.variables.descriptor_view_ids.borrow().get(&Arc::as_ptr(body)).copied();
        let path = self.normalizing_nominals.borrow();
        let cacheable = path.len() <= 1;
        let context = path.first().cloned();
        drop(path);
        #[cfg(feature = "inference-profile")]
        if id.is_none() { profile_increment(&self.variables.profile.body_unindexed); }
        if cacheable && let Some(id) = id
            && let Some(normalized) = self.variables.normalized_body(id, context.as_ref())
        {
            return normalized;
        }
        let normalized = if contains_type_variable(body) {
            Arc::new(self.normalize(body))
        } else {
            Arc::clone(body)
        };
        if cacheable && let Some(id) = id {
            self.variables.cache_normalized_body(id, Arc::clone(&normalized), context);
        }
        normalized
    }

    fn occurs(&self, variable: InferenceVariableId, ty: &TypeDescriptor) -> bool {
        let variable = self.variables.root(variable);
        let mut pending = vec![ty];
        let mut slots = Vec::new();
        let mut visited = HashSet::new();
        while let Some(ty) = pending.pop() {
            match ty {
                TypeDescriptor::Inference(candidate) => {
                    slots.push(*candidate);
                }
                TypeDescriptor::Declared(declared) => {
                    pending.extend(declared.id.arguments());
                    pending.push(&declared.body);
                }
                TypeDescriptor::Array(item) | TypeDescriptor::Newtype(item)
                | TypeDescriptor::Dict(item) | TypeDescriptor::TypeOf(item)
                | TypeDescriptor::Tagged { payload: item, .. } => pending.push(item),
                TypeDescriptor::Tuple(items) | TypeDescriptor::PendingAlternatives(items) => pending.extend(items),
                TypeDescriptor::Struct(fields) => pending.extend(fields.values()),
                TypeDescriptor::Enum(variants) => pending.extend(variants.values().filter_map(Option::as_deref)),
                TypeDescriptor::Function { parameters, result } => {
                    pending.extend(parameters);
                    pending.push(result);
                }
                _ => {}
            }
        }
        while let Some(candidate) = slots.pop() {
            let candidate = self.variables.root(candidate);
            if candidate == variable { return true; }
            if visited.insert(candidate)
                && let Some(id) = self.variables.known(candidate)
            {
                slots.extend_from_slice(self.variables.arguments(id));
            }
        }
        false
    }
}
