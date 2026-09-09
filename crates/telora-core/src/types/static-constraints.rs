impl StaticContractScope<'_> {
    // Constraint facts depend on trait identity and property type shape, not on
    // a property provider's value. No evaluator or runtime metadata enters here.
    fn constraints(
        &self,
        authored: &[Vec<Expr>],
        traits: &BTreeMap<String, crate::TraitId>,
        graph: &mut TypeGraph,
    ) -> Option<Vec<TypeConstraint>> {
        let mut constraints = Vec::new();
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
                    let root = self.elaborate(property, graph)?;
                    TypeCapability::Property(graph.descriptor(root).ok()?)
                } else {
                    return None;
                };
                constraints.push(TypeConstraint {
                    parameter: parameter.id,
                    capability,
                    location: bound.location,
                });
            }
        }
        Some(constraints)
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

#[cfg(test)]
mod static_constraint_tests {
    use super::*;

    #[test]
    fn property_and_trait_constraints_require_no_execution_fuel() {
        let analysis = analyze_source_with_fuel("constraints", "trait Display { display: Fn(Self) -> String }; type Label = struct {text: String}; type Box(T: Property(Label)) = Array(T); def identity: for(T: Property(Label) + Display) Fn(T) -> T = fn(value) { value }; export {identity, Box};", 0).unwrap();
        let constraints = &analysis.module_interface.exports["identity"].constraints;
        assert!(constraints.iter().any(|constraint| matches!(&constraint.capability, TypeCapability::Property(TypeDescriptor::Declared(declared)) if declared.name == "Label")));
        assert!(constraints.iter().any(|constraint| matches!(&constraint.capability, TypeCapability::Trait { id, .. } if *id == analysis.trait_ids["Display"])));
        assert!(matches!(
            analysis.module_interface.exports["Box"].constraints[0].capability,
            TypeCapability::Property(_)
        ));
    }

    #[test]
    fn static_duplicate_property_constraints_keep_the_original_diagnostic() {
        let error = analyze_source_with_fuel("constraints", "type Label = struct {text: String}; def identity: for(T: Property(Label) + Property(Label)) Fn(T) -> T = fn(value) { value };", 0).unwrap_err();
        assert!(
            error
                .message
                .contains("duplicate type parameter constraint"),
            "{error}"
        );
    }
}
