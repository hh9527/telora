impl<'a> GenericInference<'a> {
    fn is_builtin_tuple(&self, expression: &Expr) -> bool {
        self.hir
            .expression_ids_at(expression.location)
            .filter_map(|id| self.hir.expression(id))
            .filter_map(|expression| expression.reference)
            .filter_map(|id| self.hir.reference(id))
            .any(|reference| {
                reference.resolution == HirResolution::External
                    && (reference.name == "\0telora_tuple_type"
                        || (self.builtin_tuple_available && reference.name == "Tuple"))
            })
    }

    fn infer_block(
        &mut self,
        block: &Block,
        environment: &dyn TypeEnvironment,
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
        environment: &dyn TypeEnvironment,
        expected: Option<&TypeDescriptor>,
    ) -> Result<TypeDescriptor, String> {
        let mut environment = ScopedTypeEnvironment::new(environment);
        let mut diverges = false;
        let declared_contracts = block
            .value
            .bindings
            .iter()
            .filter(|binding| matches!(binding.value.kind, BindingKind::Decl | BindingKind::Def))
            .filter_map(|binding| {
                binding
                    .value
                    .annotation
                    .as_ref()
                    .and_then(|annotation| self.local_annotations.get(&annotation.location))
                    .cloned()
                    .map(|contract| (binding.value.name.value.clone(), (binding.value.name.location, contract)))
            })
            .collect::<HashMap<_, _>>();
        for (name, (location, contract)) in &declared_contracts {
            self.bind_local(&mut environment, *location, name, contract.clone(), None);
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
            let first_owned_variable = self.variables.next_id();
            if let Some(skeleton) = self.recursive_closure_skeleton(&binding.value.value) {
                self.bind_local(&mut environment, binding.value.name.location, &binding.value.name.value, skeleton.clone(), None);
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
            if binding.value.kind != BindingKind::Def {
                continue;
            }
            self.delayed_initializer_depth += 1;
            self.recursive_body_inference_depth += 1;
            let inferred = self.infer(
                &binding.value.value,
                &environment,
                Some(skeleton),
            );
            self.recursive_body_inference_depth -= 1;
            self.delayed_initializer_depth -= 1;
            let inferred = inferred?;
            let inferred = (*self.variables.head(&inferred)).clone();
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
            let first_owned_variable = self.variables.next_id();
            self.delayed_initializer_depth += 1;
            let inferred = self.infer(&binding.value.value, &environment, None);
            self.delayed_initializer_depth -= 1;
            let inferred = inferred?;
            diverges |= matches!(&*self.variables.head(&inferred), TypeDescriptor::Never);
            let scheme = self.generalize_local_closure(
                &inferred,
                first_owned_variable,
                binding.value.name.location,
                binding.value.value.location,
            )?;
            let descriptor = scheme
                .as_ref()
                .map_or_else(|| inferred.clone(), |scheme| scheme.body.clone());
            self.bind_local(&mut environment, binding.value.name.location, &binding.value.name.value, descriptor, scheme.clone());
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
                .and_then(|annotation| self.local_annotations.get(&annotation.location)).cloned();
            let binding_expected = annotated_expected.as_ref().or_else(|| {
                declared_contracts
                    .get(&binding.value.name.value)
                    .map(|(_, contract)| contract)
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
                .map_or(self.variables.next_id(), |(_, first)| *first);
            if is_delayed {
                self.delayed_initializer_depth += 1;
            }
            let inferred = if matches!(binding.value.kind, BindingKind::Type | BindingKind::Trait) {
                self.infer(&binding.value.value, &environment, binding_expected)
            } else {
                self.infer(&binding.value.value, &environment, binding_expected)
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
            diverges |= matches!(&*self.variables.head(&inferred), TypeDescriptor::Never);
            if matches!(
                binding.value.kind,
                BindingKind::Let | BindingKind::Def | BindingKind::Impl | BindingKind::Import
            ) {
                let inferred_scheme = if binding.value.is_member_import() {
                    Some(self.member_import_scheme(&binding.value, &inferred)?)
                } else if binding.value.kind == BindingKind::Let
                    && binding.value.annotation.is_none()
                    && binding.value.type_parameters.is_empty()
                    && matches!(binding.value.value.value, ExprKind::Closure { .. })
                {
                    self.generalize_local_closure(
                        &inferred,
                        first_owned_variable,
                        binding.value.name.location,
                        binding.value.value.location,
                    )?
                } else {
                    None
                };
                let descriptor = inferred_scheme.as_ref().map_or_else(
                    || binding_expected.cloned().unwrap_or(inferred),
                    |scheme| scheme.body.clone(),
                );
                self.bind_local(&mut environment, binding.value.name.location, &binding.value.name.value, descriptor.clone(), inferred_scheme.clone());
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
        let result = self.infer(
            &block.value.result,
            &environment,
            if diverges { None } else { expected },
        )?;
        for (name, descriptor, first_owned_variable) in delayed {
            if let Some(query) = &self.query {
                query.check().map_err(|error| error.to_string())?;
            }
            if self.contains_owned_unknown(&descriptor, first_owned_variable) {
                return Err(format!(
                    "cannot infer monomorphic binding {name:?}: unresolved {}",
                    self.normalize(&descriptor).display_name()
                ));
            }
        }
        Ok(if diverges {
            TypeDescriptor::Never
        } else {
            result
        })
    }
}
