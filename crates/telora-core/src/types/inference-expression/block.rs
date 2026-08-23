impl<'a> GenericInference<'a> {
    fn is_builtin_tuple(&self, expression: &Expr) -> bool {
        if !self.builtin_tuple_available {
            return false;
        }
        self.hir
            .expression_ids_at(expression.location)
            .filter_map(|id| self.hir.expression(id))
            .filter_map(|expression| expression.reference)
            .filter_map(|id| self.hir.reference(id))
            .any(|reference| {
                reference.name == "Tuple" && reference.resolution == HirResolution::External
            })
    }

    fn infer_block(
        &mut self,
        block: &Block,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        self.scheme_scopes.push(HashMap::new());
        let result = self.infer_block_scoped(block, environment, expected);
        self.scheme_scopes.pop();
        result
    }

    fn infer_block_scoped(
        &mut self,
        block: &Block,
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        let mut environment = environment.clone();
        for binding in &block.value.bindings {
            if binding
                .value
                .annotation
                .as_ref()
                .and_then(|annotation| self.local_annotations.get(&annotation.location))
                .is_some_and(contains_any_descriptor)
            {
                self.authored_any_definitions
                    .insert(binding.value.name.location);
            }
        }
        let declared_contracts = block
            .value
            .bindings
            .iter()
            .filter(|binding| binding.value.kind == BindingKind::Decl)
            .filter_map(|binding| {
                binding
                    .value
                    .annotation
                    .as_ref()
                    .and_then(|annotation| self.local_annotations.get(&annotation.location))
                    .cloned()
                    .map(|contract| (binding.value.name.value.clone(), contract))
            })
            .collect::<HashMap<_, _>>();
        for (name, contract) in &declared_contracts {
            environment.insert(name.clone(), contract.clone());
            self.set_local_scheme(name.clone(), None);
        }
        let mut delayed = Vec::new();
        let mut recursive_skeletons = HashMap::new();
        let component_plan = definition_component_plan(block, self.hir);
        if !component_plan.indirect_recursive.is_empty() {
            return Err("indirect recursive definition requires an explicit contract".into());
        }
        for binding in &block.value.bindings {
            if binding.value.kind != BindingKind::Def || binding.value.annotation.is_some() {
                continue;
            }
            if !component_plan
                .recursive
                .contains(&binding.value.name.location)
            {
                continue;
            }
            let first_owned_variable = self.next_variable;
            if let Some(skeleton) = self.recursive_closure_skeleton(&binding.value.value) {
                environment.insert(binding.value.name.value.clone(), skeleton.clone());
                self.set_local_scheme(binding.value.name.value.clone(), None);
                recursive_skeletons.insert(
                    binding.value.name.value.clone(),
                    (skeleton.clone(), first_owned_variable),
                );
                delayed.push((
                    binding.value.name.value.clone(),
                    skeleton,
                    first_owned_variable,
                ));
            }
        }
        let recursive_variables = recursive_skeletons
            .values()
            .filter_map(|(skeleton, _)| Self::recursive_result_variable(skeleton))
            .collect::<HashSet<_>>();
        for binding in &block.value.bindings {
            let Some((skeleton, _)) = recursive_skeletons.get(&binding.value.name.value) else {
                continue;
            };
            self.delayed_initializer_depth += 1;
            self.recursive_body_inference_depth += 1;
            let recursive_expected = Self::recursive_expected(skeleton);
            let inferred = self.infer(
                &binding.value.value,
                &environment,
                Some(&recursive_expected),
            );
            self.recursive_body_inference_depth -= 1;
            self.delayed_initializer_depth -= 1;
            let inferred = inferred?;
            if let (
                Some(variable),
                TypeDescriptor::Function {
                    result: inferred_result,
                    ..
                },
            ) = (Self::recursive_result_variable(skeleton), inferred)
            {
                self.recursive_equations.insert(variable, *inferred_result);
            }
        }
        self.solve_recursive_equations(&recursive_variables)?;
        for location in &component_plan.acyclic {
            let binding = block
                .value
                .bindings
                .iter()
                .find(|binding| binding.value.name.location == *location)
                .expect("component binding exists");
            let first_owned_variable = self.next_variable;
            self.delayed_initializer_depth += 1;
            let inferred = self.infer(&binding.value.value, &environment, None);
            self.delayed_initializer_depth -= 1;
            let inferred = inferred?;
            let scheme = self.generalize_local_closure(
                &inferred,
                first_owned_variable,
                binding.value.name.location,
            )?;
            let descriptor = scheme
                .as_ref()
                .map_or_else(|| self.resolve(&inferred), |scheme| scheme.body.clone());
            environment.insert(binding.value.name.value.clone(), descriptor);
            self.set_local_scheme(binding.value.name.value.clone(), scheme.clone());
            if let Some(scheme) = scheme {
                self.inferred_schemes
                    .insert(binding.value.name.location, scheme);
            } else {
                delayed.push((
                    binding.value.name.value.clone(),
                    inferred,
                    first_owned_variable,
                ));
            }
        }
        for binding in &block.value.bindings {
            if binding.value.kind == BindingKind::Decl {
                continue;
            }
            if recursive_skeletons.contains_key(&binding.value.name.value) {
                continue;
            }
            if component_plan
                .acyclic
                .contains(&binding.value.name.location)
            {
                continue;
            }
            let annotated_expected = binding
                .value
                .annotation
                .as_ref()
                .and_then(|annotation| self.local_annotations.get(&annotation.location));
            let binding_expected = annotated_expected.or_else(|| {
                declared_contracts
                    .get(&binding.value.name.value)
                    .or_else(|| {
                        recursive_skeletons
                            .get(&binding.value.name.value)
                            .map(|(skeleton, _)| skeleton)
                    })
            });
            let is_recursive = recursive_skeletons.contains_key(&binding.value.name.value);
            if binding.value.kind == BindingKind::Def
                && binding.value.annotation.is_none()
                && !is_recursive
                && !declared_contracts.contains_key(&binding.value.name.value)
                && expression_references_names(
                    &binding.value.value,
                    &HashSet::from([binding.value.name.value.clone()]),
                    &HashSet::new(),
                )
            {
                return Err(format!(
                    "recursive definition {:?} requires a closure value or explicit contract",
                    binding.value.name.value
                ));
            }
            let is_delayed = (annotated_expected.is_none() || is_recursive)
                && matches!(binding.value.kind, BindingKind::Let | BindingKind::Def);
            let first_owned_variable = recursive_skeletons
                .get(&binding.value.name.value)
                .map_or(self.next_variable, |(_, first)| *first);
            if is_delayed {
                self.delayed_initializer_depth += 1;
            }
            let inferred = if binding.value.kind == BindingKind::Type {
                self.infer(&binding.value.value, &environment, binding_expected)
            } else {
                self.infer_authored_boundary(&binding.value.value, &environment, binding_expected)
            };
            if is_delayed {
                self.delayed_initializer_depth -= 1;
            }
            if inferred.is_err() {
                let expected_location = binding
                    .value
                    .annotation
                    .as_ref()
                    .map(|annotation| annotation.location)
                    .or_else(|| {
                        block.value.bindings.iter().find_map(|candidate| {
                            (candidate.value.kind == BindingKind::Decl
                                && candidate.value.name.value == binding.value.name.value)
                                .then(|| {
                                    candidate
                                        .value
                                        .annotation
                                        .as_ref()
                                        .map(|annotation| annotation.location)
                                })
                                .flatten()
                        })
                    });
                self.record_failure_expected_location(expected_location);
            }
            let inferred = inferred?;
            if matches!(
                binding.value.kind,
                BindingKind::Let | BindingKind::Def | BindingKind::Import
            ) {
                let inferred_scheme = if binding.value.kind == BindingKind::Let
                    && binding.value.annotation.is_none()
                    && binding.value.type_parameters.is_empty()
                    && matches!(binding.value.value.value, ExprKind::Closure { .. })
                {
                    self.generalize_local_closure(
                        &inferred,
                        first_owned_variable,
                        binding.value.name.location,
                    )?
                } else {
                    None
                };
                let descriptor = inferred_scheme.as_ref().map_or_else(
                    || binding_expected.cloned().unwrap_or(inferred),
                    |scheme| scheme.body.clone(),
                );
                environment.insert(binding.value.name.value.clone(), descriptor.clone());
                self.set_local_scheme(binding.value.name.value.clone(), inferred_scheme.clone());
                if let Some(scheme) = &inferred_scheme {
                    self.inferred_schemes
                        .insert(binding.value.name.location, scheme.clone());
                }
                if is_delayed && !is_recursive && inferred_scheme.is_none() {
                    delayed.push((
                        binding.value.name.value.clone(),
                        descriptor,
                        first_owned_variable,
                    ));
                }
            }
        }
        let result = self.infer_authored_boundary(&block.value.result, &environment, expected)?;
        for (name, descriptor, first_owned_variable) in delayed {
            if let Some(query) = &self.query {
                query.check().map_err(|error| error.to_string())?;
            }
            let resolved = self.resolve(&descriptor);
            if contains_inference_variable_at_or_after(&resolved, first_owned_variable) {
                return Err(format!(
                    "cannot infer monomorphic binding {name:?}: unresolved {}",
                    resolved.display_name()
                ));
            }
        }
        Ok(self.resolve(&result))
    }
}
