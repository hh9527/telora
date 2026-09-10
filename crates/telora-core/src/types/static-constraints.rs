struct StaticConstraintSolution {
    known: Vec<TypeConstraint>,
    unknown: Vec<Diagnostic>,
}

impl StaticContractScope<'_> {
    fn check_contract_obligations(
        &self, expression: &Expr, inference: &mut GenericInference<'_>, environment: &dyn TypeEnvironment,
    ) -> Result<(), (crate::Location, String)> {
        match &expression.value {
            ExprKind::TypeSyntax(inner) => self.check_contract_obligations(inner, inference, environment)?,
            ExprKind::Call { callee, arguments } => {
                if self.family_name(callee).and_then(|name| self.families.get(&name))
                    .is_some_and(|family| family.has_constraints)
                {
                    inference.type_syntax_depth += 1;
                    let result = inference.infer(expression, environment, Some(&TypeDescriptor::Type));
                    inference.type_syntax_depth -= 1;
                    result.map_err(|message| (expression.location, message))?;
                } else {
                    for argument in arguments {
                        self.check_contract_obligations(argument, inference, environment)?;
                    }
                }
            }
            ExprKind::Array(items) => for item in items {
                self.check_contract_obligations(item, inference, environment)?;
            },
            ExprKind::Dict(fields) => for field in fields {
                self.check_contract_obligations(&field.value.value, inference, environment)?;
            },
            _ => {}
        }
        Ok(())
    }

    // Constraint facts depend on trait identity and property type shape, not on
    // a property provider's value. No evaluator or runtime metadata enters here.
    fn constraints(
        &self,
        authored: &[Vec<Expr>],
        traits: &BTreeMap<String, crate::TraitId>,
        graph: &mut TypeGraph,
    ) -> StaticConstraintSolution {
        let mut constraints = Vec::new();
        let mut unknown = Vec::new();
        for (parameter, bounds) in self.parameters.iter().zip(authored) {
            for bound in bounds {
                let capability = if let Some((id, name)) =
                    visible_trait(bound, traits, self.interfaces)
                {
                    TypeCapability::Trait { id, name }
                } else if let ExprKind::Call { callee, arguments } = &bound.value
                    && matches!(&callee.value, ExprKind::Variable(name) if name.value == "Property")
                    && let [property] = arguments.as_slice()
                {
                    let Some(descriptor) = self.elaborate(property, graph)
                        .and_then(|root| graph.descriptor(root).ok()) else {
                        unknown.push(Diagnostic::error("property constraint type remains unknown", property.location));
                        continue;
                    };
                    TypeCapability::Property(descriptor)
                } else {
                    unknown.push(Diagnostic::error("unknown trait or constraint", bound.location));
                    continue;
                };
                constraints.push(TypeConstraint {
                    parameter: parameter.id,
                    capability,
                    location: bound.location,
                });
            }
        }
        StaticConstraintSolution { known: constraints, unknown }
    }
}

fn finish_type_constraints(
    mut constraints: Vec<TypeConstraint>,
    sources: &SourceDatabase,
) -> Result<Vec<TypeConstraint>, FrontendError> {
    for (index, constraint) in constraints.iter().enumerate() {
        if constraints[..index].iter().any(|previous| {
            previous.parameter == constraint.parameter
                && previous.capability == constraint.capability
        }) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error("duplicate type parameter constraint", constraint.location),
            ));
        }
    }
    constraints.sort_by(|left, right| {
        left.parameter.cmp(&right.parameter).then_with(|| {
            left.capability
                .display_name()
                .cmp(&right.capability.display_name())
        })
    });
    Ok(constraints)
}
