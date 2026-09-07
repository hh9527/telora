impl<'a> GenericInference<'a> {
    fn new(
        schemes: &HashMap<String, TypeScheme>,
        hir: &'a HirProgram,
        external_interfaces: &'a BTreeMap<String, ModuleInterface>,
        named_types: &'a BTreeMap<String, TypeDescriptor>,
        local_annotations: &'a HashMap<crate::Location, TypeDescriptor>,
        trait_implementations: &'a [TraitImplementation],
        type_properties: &'a [TypePropertyEvidence],
        trait_ids: &'a BTreeMap<String, crate::TraitId>,
        display_trait: Option<(crate::TraitId, String)>,
        dyn_namespaces: &'a HashSet<String>,
        builtin_tuple_available: bool,
        query: Option<crate::query::QueryContext>,
    ) -> Self {
        let mut declared_bodies = HashMap::new();
        for scheme in schemes.values() {
            collect_declared_bodies(&scheme.body, &mut declared_bodies, &mut HashSet::new());
        }
        for descriptor in local_annotations.values() {
            collect_declared_bodies(descriptor, &mut declared_bodies, &mut HashSet::new());
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
        Self {
            schemes: schemes.clone(),
            scheme_scopes: vec![HashMap::new()],
            top_level_inferred_schemes: HashMap::new(),
            inferred_schemes: HashMap::new(),
            placeholder_obligations: Vec::new(),
            pending_type_constraints: Vec::new(),
            trait_implementations,
            type_properties,
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
            local_annotations,
            dyn_namespaces,
            builtin_tuple_available,
            query,
            next_variable: 0,
            closure_inference_depth: 0,
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
            substitutions: HashMap::new(),
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

    fn take_failure_location(&mut self, fallback: crate::Location) -> crate::Location {
        self.failure_location.take().unwrap_or(fallback)
    }

    fn infer_pattern_constructors(
        &mut self,
        pattern: &crate::ast::Pattern,
        matched: &TypeDescriptor,
        environment: &HashMap<String, TypeDescriptor>,
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
        if let PatternKind::Constructor { constructor, payload } = &pattern.value {
            self.failure_location = Some(constructor.location);
            let ty = self.infer(constructor, environment, None)?;
            let kind = self.value_constructors.get(&constructor.location).cloned()
                .ok_or_else(|| "constructor pattern requires a type declaration".to_owned())?;
            let resolved = self.resolve(&ty);
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
                    payload, &self.resolve(&ty), environment,
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
        let body = self.declared_body(declared);
        Some(TypeDescriptor::Declared(DeclaredTypeDescriptor {
            id: declared.id.clone(),
            name: declared.name.clone(),
            body: Arc::new(body.clone()),
        }))
    }

    fn expose_named(&self, ty: &TypeDescriptor) -> TypeDescriptor {
        let mut current = self.resolve(ty);
        let mut visited = HashSet::new();
        while let TypeDescriptor::Named(name) = &current {
            if !visited.insert(name.clone()) {
                break;
            }
            let Some(target) = self.named_type(name) else {
                break;
            };
            current = self.resolve(target);
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
        fn find(
            inference: &GenericInference<'_>,
            ty: &TypeDescriptor,
            named: &mut HashSet<String>,
            variables: &mut HashSet<InferenceVariableId>,
        ) -> Option<crate::value::DeclaredTypeId> {
            match ty {
                TypeDescriptor::Declared(declared) => Some(declared.id.clone()),
                TypeDescriptor::Named(name) if named.insert(name.clone()) => inference
                    .named_type(name)
                    .and_then(|ty| find(inference, ty, named, variables)),
                TypeDescriptor::Inference(variable) if variables.insert(*variable) => inference
                    .substitutions
                    .get(variable)
                    .and_then(|ty| find(inference, ty, named, variables)),
                _ => None,
            }
        }

        find(self, ty, &mut HashSet::new(), &mut HashSet::new())
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
        Ok(common_type(values).unwrap_or(TypeDescriptor::Never))
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
        let resolved = self.resolve(&result);
        match requirement {
            PropagationRequirement::Option => match resolved {
                TypeDescriptor::Inference(_) | TypeDescriptor::PendingAlternatives(_) => {
                    let success = self.fresh_variable();
                    let target = option_descriptor(success);
                    self.check(&resolved, &target)?;
                    Ok(self.resolve(&target))
                }
                TypeDescriptor::Enum(ref variants) if option_parts(variants).is_some() => {
                    Ok(resolved)
                }
                TypeDescriptor::Tagged { tag, payload } if tag.name() == "Some" => {
                    Ok(option_descriptor(*payload))
                }
                TypeDescriptor::Atom(tag) if tag.name() == "None" => {
                    match expected.map(|ty| self.resolve(ty)) {
                        Some(ref expected @ TypeDescriptor::Enum(ref variants)) if option_parts(variants).is_some() => Ok(expected.clone()),
                        _ => Err("Option propagation boundary ending in 'None needs an expected Option success type".into()),
                    }
                }
                _ => Err(format!(
                    "Option propagation requires an Option-shaped boundary result, found {}",
                    resolved.display_name()
                )),
            },
            PropagationRequirement::Result(errors) => {
                let expected = expected.map(|ty| self.resolve(ty));
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
                    TypeDescriptor::Inference(_) | TypeDescriptor::PendingAlternatives(_) => {
                        let success = self.fresh_variable();
                        let target = result_descriptor(success, boundary_error);
                        self.check(&resolved, &target)?;
                        Ok(self.resolve(&target))
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
                            _ => Err("Result propagation boundary ending in 'Err(_) needs an expected Result success type".into()),
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
                let variable = InferenceVariableId(self.next_variable);
                self.next_variable += 1;
                (parameter, variable)
            })
            .collect();
        for constraint in &scheme.constraints {
            if let Some(variable) = variables.get(&constraint.parameter).copied() {
                let capability = match &constraint.capability {
                    TypeCapability::Trait { id, name } => TypeCapability::Trait {
                        id: *id,
                        name: name.clone(),
                    },
                    TypeCapability::Property(property) => TypeCapability::Property(
                        self.instantiate_with(property, &mut variables),
                    ),
                };
                self.pending_type_constraints.push(PendingTypeConstraint {
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

    fn namespace_interface(&self, expression: &Expr) -> Option<&ModuleInterface> {
        match &expression.value {
            ExprKind::Variable(name) => self.external_interfaces.get(&name.value)
                .filter(|interface| interface.value_binding.is_none()),
            ExprKind::Field { receiver, field } => self.namespace_interface(receiver)?.namespaces.get(&field.value),
            _ => None,
        }
    }

    fn explicit_scheme(&self, callee: &Expr) -> Option<TypeScheme> {
        match &callee.value {
            ExprKind::Variable(name) => self.member_import_definition(name)
                .and_then(|definition| self.inferred_schemes.get(&definition.location).cloned()
                    .or_else(|| self.explicit_scheme(definition.member_import.as_ref()?)))
                .or_else(|| self.scheme(&name.value)),
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
        let reference = self.hir.references().iter()
            .find(|reference| reference.location == name.location && reference.name == name.value)?;
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
                if self.hir.references().iter().any(|reference|
                    reference.location == name.location && reference.name == name.value
                        && matches!(reference.resolution, HirResolution::Definition(id)
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
                if matches!(name.value.as_str(), "Bool" | "Option" | "Result" | "FoldControl")
                    && !self.external_interfaces.contains_key(&name.value)
                    && self.hir.references().iter().any(|reference|
                        reference.location == name.location && reference.name == name.value
                            && reference.resolution == HirResolution::External)
                {
                    return true;
                }
                if let Some(reference) = self.hir.references().iter()
                    .find(|reference| reference.location == name.location && reference.name == name.value)
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
        let variable = InferenceVariableId(self.next_variable);
        self.next_variable += 1;
        TypeDescriptor::Inference(variable)
    }

    fn freshen_join_context(
        &mut self,
        expected: &TypeDescriptor,
        environment: &HashMap<String, TypeDescriptor>,
    ) -> (
        TypeDescriptor,
        HashMap<String, TypeDescriptor>,
        HashMap<InferenceVariableId, InferenceVariableId>,
    ) {
        let expected = self.resolve(expected);
        let mut variables = Vec::new();
        collect_inference_variables(&expected, &mut variables);
        let replacements = variables
            .into_iter()
            .map(|variable| {
                let TypeDescriptor::Inference(fresh) = self.fresh_variable() else {
                    unreachable!("fresh variable descriptor")
                };
                if self.numeric_variables.contains(&variable) {
                    self.numeric_variables.insert(fresh);
                }
                if self.not_variables.contains(&variable) {
                    self.not_variables.insert(fresh);
                }
                if self.ordered_variables.contains(&variable) {
                    self.ordered_variables.insert(fresh);
                }
                (variable, fresh)
            })
            .collect::<HashMap<_, _>>();
        let expected = replace_inference_variables(&expected, &replacements);
        let environment = environment
            .iter()
            .map(|(name, descriptor)| {
                (
                    name.clone(),
                    replace_inference_variables(&self.resolve(descriptor), &replacements),
                )
            })
            .collect();
        (expected, environment, replacements)
    }

    fn merge_join_evidence(
        &mut self,
        branches: &[HashMap<InferenceVariableId, InferenceVariableId>],
    ) -> Result<(), String> {
        let Some(first) = branches.first() else {
            return Ok(());
        };
        let originals = first.keys().copied().collect::<Vec<_>>();
        for original in originals {
            let mut evidence = Vec::new();
            for branch in branches {
                let Some(fresh) = branch.get(&original) else {
                    continue;
                };
                let resolved = self.resolve(&TypeDescriptor::Inference(*fresh));
                if !contains_type_variable(&resolved) {
                    evidence.push(resolved);
                }
            }
            if !evidence.is_empty() {
                let joined = join_all_types(evidence);
                self.check(&joined, &TypeDescriptor::Inference(original))?;
                let merged = self.resolve(&TypeDescriptor::Inference(original));
                for branch in branches {
                    let Some(fresh) = branch.get(&original) else {
                        continue;
                    };
                    if contains_type_variable(&self.resolve(&TypeDescriptor::Inference(*fresh))) {
                        self.check(&merged, &TypeDescriptor::Inference(*fresh))?;
                    }
                }
            }
        }
        Ok(())
    }

    fn merge_structural_join_evidence(
        &mut self,
        branches: &[TypeDescriptor],
    ) -> Result<(), String> {
        fn collect(
            unresolved: &TypeDescriptor,
            evidence: &TypeDescriptor,
            collected: &mut HashMap<InferenceVariableId, Vec<TypeDescriptor>>,
            collection_element: bool,
            enum_owners: &HashSet<InferenceVariableId>,
        ) {
            if let TypeDescriptor::Inference(variable) = unresolved {
                if (collection_element || enum_owners.contains(variable))
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
                    collect(left, right, collected, collection_element, enum_owners);
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
                    collect(left, right, collected, collection_element, enum_owners);
                }
                (TypeDescriptor::Tuple(left), TypeDescriptor::Tuple(right))
                    if left.len() == right.len() =>
                {
                    for (left, right) in left.iter().zip(right) {
                        collect(left, right, collected, collection_element, enum_owners);
                    }
                }
                (TypeDescriptor::Struct(left), TypeDescriptor::Struct(right))
                    if left.keys().eq(right.keys()) =>
                {
                    for (name, left) in left {
                        collect(left, &right[name], collected, collection_element, enum_owners);
                    }
                }
                (TypeDescriptor::Enum(left), TypeDescriptor::Enum(right))
                    if left.keys().eq(right.keys()) =>
                {
                    for (name, left) in left {
                        if let (Some(left), Some(right)) = (left.as_deref(), right[name].as_deref())
                        {
                            collect(left, right, collected, collection_element, enum_owners);
                        }
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
                        collect(left, right, collected, collection_element, enum_owners);
                    }
                    collect(left_result, right_result, collected, collection_element, enum_owners);
                }
                _ => {}
            }
        }

        let resolved = branches
            .iter()
            .map(|branch| self.resolve(branch))
            .collect::<Vec<_>>();
        let mut collected = HashMap::new();
        let enum_owners = self.enum_constructors.keys().copied().collect();
        for (index, branch) in resolved.iter().enumerate() {
            for evidence in resolved.iter().skip(index + 1) {
                collect(branch, evidence, &mut collected, false, &enum_owners);
                collect(evidence, branch, &mut collected, false, &enum_owners);
            }
        }
        for (variable, evidence) in collected {
            self.check(
                &join_all_types(evidence),
                &TypeDescriptor::Inference(variable),
            )?;
        }
        Ok(())
    }

    fn generalize_local_closure(
        &mut self,
        descriptor: &TypeDescriptor,
        first_owned_variable: u32,
        location: crate::Location,
    ) -> Result<Option<TypeScheme>, String> {
        let descriptor = self.resolve(descriptor);
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
        let parameters = variables
            .iter()
            .enumerate()
            .map(|(index, _)| TypeParameter {
                id: TypeParameterId(first_parameter + index as u32),
                name: inferred_type_parameter_name(index),
                location,
            })
            .collect();
        for descriptor in self.records.values_mut() {
            *descriptor = bind_inference_variables(descriptor, &replacements);
        }
        for descriptor in self.pattern_binding_types.values_mut() {
            *descriptor = bind_inference_variables(descriptor, &replacements);
        }
        Ok(Some(TypeScheme {
            parameters,
            constraints: Vec::new(),
            body: bind_inference_variables(&descriptor, &replacements),
        }))
    }

    fn unresolved_placeholder_since(&self, start: usize) -> Option<(crate::Location, String)> {
        self.placeholder_obligations[start..]
            .iter()
            .find_map(|(variable, location, parameter)| {
                contains_type_variable(&self.resolve(&TypeDescriptor::Inference(*variable))).then(
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
                let resolved = self.resolve(descriptor);
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
        match ty {
            TypeDescriptor::Bound(parameter) => variables
                .get(parameter)
                .map_or_else(|| ty.clone(), |fresh| TypeDescriptor::Inference(*fresh)),
            TypeDescriptor::Declared(declared) => {
                let arguments = declared
                    .id
                    .arguments()
                    .iter()
                    .map(|argument| self.instantiate_with(argument, variables))
                    .collect::<Vec<_>>();
                TypeDescriptor::Declared(DeclaredTypeDescriptor {
                    id: declared.id.reapply(&arguments),
                    name: declared.name.clone(),
                    body: if arguments.is_empty() {
                        Arc::clone(&declared.body)
                    } else {
                        Arc::new(self.instantiate_with(&declared.body, variables))
                    },
                })
            }
            TypeDescriptor::Array(item) => {
                TypeDescriptor::Array(Box::new(self.instantiate_with(item, variables)))
            }
            TypeDescriptor::Newtype(item) => {
                TypeDescriptor::Newtype(Box::new(self.instantiate_with(item, variables)))
            }
            TypeDescriptor::Dict(item) => {
                TypeDescriptor::Dict(Box::new(self.instantiate_with(item, variables)))
            }
            TypeDescriptor::TypeOf(instance) => {
                TypeDescriptor::TypeOf(Box::new(self.instantiate_with(instance, variables)))
            }
            TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
                tag: tag.clone(),
                payload: Box::new(self.instantiate_with(payload, variables)),
            },
            TypeDescriptor::Tuple(items) => TypeDescriptor::Tuple(
                items
                    .iter()
                    .map(|item| self.instantiate_with(item, variables))
                    .collect(),
            ),
            TypeDescriptor::Struct(fields) => {
                let mut instantiated = fields.clone();
                for (source, target) in fields.values().zip(instantiated.values_mut()) {
                    *target = self.instantiate_with(source, variables);
                }
                TypeDescriptor::Struct(instantiated)
            }
            TypeDescriptor::Enum(variants) => {
                let mut instantiated = variants.clone();
                for (source, target) in variants.values().zip(instantiated.values_mut()) {
                    *target = source
                        .as_ref()
                        .map(|payload| Box::new(self.instantiate_with(payload, variables)));
                }
                TypeDescriptor::Enum(instantiated)
            }
            TypeDescriptor::PendingAlternatives(variants) => TypeDescriptor::PendingAlternatives(
                variants
                    .iter()
                    .map(|variant| self.instantiate_with(variant, variables))
                    .collect(),
            ),
            TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
                parameters: parameters
                    .iter()
                    .map(|parameter| self.instantiate_with(parameter, variables))
                    .collect(),
                result: Box::new(self.instantiate_with(result, variables)),
            },
            ty => ty.clone(),
        }
    }

    fn resolve(&self, ty: &TypeDescriptor) -> TypeDescriptor {
        match ty {
            TypeDescriptor::Inference(variable) => self
                .substitutions
                .get(variable)
                .map_or_else(|| ty.clone(), |ty| self.resolve(ty)),
            TypeDescriptor::Declared(declared) => {
                let arguments = declared
                    .id
                    .arguments()
                    .iter()
                    .map(|argument| self.resolve(argument))
                    .collect::<Vec<_>>();
                TypeDescriptor::Declared(DeclaredTypeDescriptor {
                    id: declared.id.reapply(&arguments),
                    name: declared.name.clone(),
                    body: if arguments.is_empty() {
                        Arc::clone(&declared.body)
                    } else {
                        Arc::new(self.resolve(&declared.body))
                    },
                })
            }
            TypeDescriptor::Array(item) => TypeDescriptor::Array(Box::new(self.resolve(item))),
            TypeDescriptor::Newtype(item) => TypeDescriptor::Newtype(Box::new(self.resolve(item))),
            TypeDescriptor::Dict(item) => TypeDescriptor::Dict(Box::new(self.resolve(item))),
            TypeDescriptor::TypeOf(instance) => {
                TypeDescriptor::TypeOf(Box::new(self.resolve(instance)))
            }
            TypeDescriptor::Tagged { tag, payload } => TypeDescriptor::Tagged {
                tag: tag.clone(),
                payload: Box::new(self.resolve(payload)),
            },
            TypeDescriptor::Tuple(items) => {
                TypeDescriptor::Tuple(items.iter().map(|item| self.resolve(item)).collect())
            }
            TypeDescriptor::Struct(fields) => {
                let mut resolved = fields.clone();
                for (source, target) in fields.values().zip(resolved.values_mut()) {
                    *target = self.resolve(source);
                }
                TypeDescriptor::Struct(resolved)
            }
            TypeDescriptor::Enum(variants) => {
                let mut resolved = variants.clone();
                for (source, target) in variants.values().zip(resolved.values_mut()) {
                    *target = source
                        .as_ref()
                        .map(|payload| Box::new(self.resolve(payload)));
                }
                TypeDescriptor::Enum(resolved)
            }
            TypeDescriptor::PendingAlternatives(variants) => {
                let variants = variants
                    .iter()
                    .map(|variant| self.resolve(variant))
                    .collect::<Vec<_>>();
                pending_alternatives(variants)
            }
            TypeDescriptor::Function { parameters, result } => TypeDescriptor::Function {
                parameters: parameters
                    .iter()
                    .map(|parameter| self.resolve(parameter))
                    .collect(),
                result: Box::new(self.resolve(result)),
            },
            ty => ty.clone(),
        }
    }

    fn occurs(&self, variable: InferenceVariableId, ty: &TypeDescriptor) -> bool {
        match self.resolve(ty) {
            TypeDescriptor::Inference(candidate) => candidate == variable,
            TypeDescriptor::Declared(declared) => {
                declared
                    .id
                    .arguments()
                    .iter()
                    .any(|argument| self.occurs(variable, argument))
                    || self.occurs(variable, &declared.body)
            }
            TypeDescriptor::Array(item) => self.occurs(variable, &item),
            TypeDescriptor::Newtype(item) => self.occurs(variable, &item),
            TypeDescriptor::Dict(item) => self.occurs(variable, &item),
            TypeDescriptor::TypeOf(instance) => self.occurs(variable, &instance),
            TypeDescriptor::Tagged { payload, .. } => self.occurs(variable, &payload),
            TypeDescriptor::Tuple(items) | TypeDescriptor::PendingAlternatives(items) => {
                items.iter().any(|item| self.occurs(variable, item))
            }
            TypeDescriptor::Struct(fields) => {
                fields.values().any(|field| self.occurs(variable, field))
            }
            TypeDescriptor::Enum(variants) => variants
                .values()
                .flatten()
                .any(|payload| self.occurs(variable, payload)),
            TypeDescriptor::Function { parameters, result } => {
                parameters
                    .iter()
                    .any(|parameter| self.occurs(variable, parameter))
                    || self.occurs(variable, &result)
            }
            _ => false,
        }
    }
}
