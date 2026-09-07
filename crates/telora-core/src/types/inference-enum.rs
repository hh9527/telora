impl<'a> GenericInference<'a> {
    fn enum_constructor(
        &mut self,
        location: crate::Location,
        tag: &str,
        payload: Option<(Option<Expr>, TypeDescriptor)>,
    ) -> TypeDescriptor {
        let owner = self.fresh_variable();
        let TypeDescriptor::Inference(variable) = owner else {
            unreachable!()
        };
        self.enum_constructors.insert(variable, vec![EnumConstructorObligation {
            location,
            tag: tag.to_owned(),
            payload,
        }]);
        owner
    }

    fn bind_enum_constructors(
        &mut self,
        variable: InferenceVariableId,
        target: &TypeDescriptor,
    ) -> Result<(), String> {
        let Some(obligations) = self.enum_constructors.get(&variable).cloned() else {
            return Ok(());
        };
        if let TypeDescriptor::Inference(target) = target {
            self.enum_constructors.remove(&variable);
            self.enum_constructors.entry(*target).or_default().extend(obligations);
            return Ok(());
        }
        if let TypeDescriptor::Function { parameters, result } = target
            && obligations.iter().all(|obligation| obligation.payload.is_none())
        {
            let [parameter] = parameters.as_slice() else {
                return Err("enum constructor function requires exactly one parameter".into());
            };
            self.enum_constructors.remove(&variable);
            for obligation in obligations {
                let owner = self.enum_constructor(
                    obligation.location,
                    &obligation.tag,
                    Some((None, parameter.clone())),
                );
                self.check(&owner, result)?;
            }
            return Ok(());
        }
        let exposed = self.expose_named(target);
        let body = match &exposed {
            TypeDescriptor::Declared(declared) => declared.body.as_ref(),
            body => body,
        };
        for obligation in &obligations {
            let TypeDescriptor::Enum(variants) = body else {
                self.failure_location = Some(obligation.location);
                return Err(format!(
                    "variant '{} requires an enum context, found {}",
                    obligation.tag, target.display_name(),
                ));
            };
            let checked = match (variants.get(&obligation.tag), &obligation.payload) {
                (Some(None), None) => Ok(()),
                (Some(Some(expected)), Some((expression, actual))) => {
                    let contextual = if let Some(expression) = expression
                        && self.records.contains_key(&expression.location) {
                        self.contextualize_authored_literal(expression, expected)?
                    } else {
                        actual.clone()
                    };
                    self.check(&contextual, expected)
                }
                (Some(Some(_)), None) => Err(format!("variant '{} requires a payload", obligation.tag)),
                (Some(None), Some(_)) => Err(format!("variant '{} does not accept a payload", obligation.tag)),
                (None, _) => Err(format!("variant '{} is not part of {}", obligation.tag, target.display_name())),
            };
            if checked.is_err() && self.failure_location.is_none() {
                self.failure_location = Some(obligation.location);
            }
            checked?;
        }
        self.enum_constructors.remove(&variable);
        Ok(())
    }

    fn finish_enum_constructors(&self) -> Result<(), (crate::Location, String)> {
        if let Some(obligation) = self.enum_constructors.values().flatten()
            .min_by_key(|obligation| obligation.location.range().start)
        {
            return Err((obligation.location, format!(
                "cannot infer enum owner for '{}; supply explicit context with .ty!(Ty) or @[Ty]",
                obligation.tag,
            )));
        }
        Ok(())
    }
}
