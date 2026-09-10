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

#[cfg(test)]
mod static_constraint_tests {
    use super::*;

    #[test]
    fn unknown_constraints_do_not_discard_other_solved_constraints() {
        let mut sources = SourceDatabase::default();
        let source = sources.add("static-constraints",
            "decl f: for(T: Missing + Property(Int) + AlsoMissing) Fn(T) -> T;");
        let program = parse_registered(&sources, source).program.unwrap();
        let binding = &program.value.body.value.bindings[0];
        let environment = BootstrapPrelude::new().types;
        let hir = HirProgram::resolve(&program, environment.keys().cloned());
        let parameters = static_contract_parameters(binding, &sources).unwrap();
        let scope = StaticContractScope {
            hir: &hir, environment: &environment, external_names: &HashSet::new(),
            interfaces: &BTreeMap::new(), parameters: &parameters, families: &BTreeMap::new(),
        };
        let solved = scope.constraints(&binding.value.type_parameter_bounds,
            &BTreeMap::new(), &mut TypeGraph::default());
        assert_eq!(solved.known.len(), 1);
        assert_eq!(solved.known[0].capability, TypeCapability::Property(TypeDescriptor::Int));
        assert_eq!(solved.unknown.len(), 2);
        for (diagnostic, index) in solved.unknown.iter().zip([0, 2]) {
            assert_eq!(diagnostic.message, "unknown trait or constraint");
            assert_eq!(diagnostic.labels[0].location, binding.value.type_parameter_bounds[0][index].location);
        }
    }

    #[test]
    fn constrained_family_applications_require_property_evidence() {
        for usage in [
            "type Missing = Box(Int);",
            "def use: Fn(Box(Int)) -> Int = fn(value) { 0 };",
        ] {
            let source = format!("type Label = struct {{text: String}}; type Box(T: Property(Label)) = Array(T); {usage}");
            let error = analyze_source("family-property-obligation", &source).unwrap_err();
            assert!(error.message.contains("Property") && error.message.contains("evidence"), "{error}");
        }
    }

    #[test]
    fn constrained_family_shapes_resolve_with_lexical_evidence_without_execution() {
        analyze_source_with_fuel("family-lexical-obligation",
            "type Label = struct {text: String}; type Box(T: Property(Label)) = Array(T); type Wrap(T: Property(Label)) = Box(T); def identity: for(T: Property(Label)) Fn(Box(T)) -> Wrap(T) = fn(value) { value };", 0).unwrap();
    }

    #[test]
    fn constrained_family_signature_preserves_trait_obligations() {
        let prelude = "trait Display { display: Fn(Self) -> String }; type Box(T: Display) = Array(T);";
        analyze_source_with_fuel("family-trait-lexical", &format!(
            "{prelude} def identity: for(T: Display) Fn(Box(T)) -> Box(T) = fn(value) {{ value }};"), 0).unwrap();
        let error = analyze_source("family-trait-missing", &format!(
            "{prelude} def use: Fn(Box(Int)) -> Int = fn(value) {{ 0 }};")).unwrap_err();
        assert!(error.message.contains("does not implement Display"), "{error}");
    }

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
