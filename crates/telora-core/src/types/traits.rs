#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraitImplementation {
    pub id: crate::TraitImplId,
    pub trait_id: crate::TraitId,
    pub target: TypeDescriptor,
    pub dictionary: String,
    pub parameters: Vec<TypeParameter>,
    pub location: crate::Location,
}

fn outer_nominal_constructor(descriptor: &TypeDescriptor) -> Option<crate::TypeConstructorId> {
    match descriptor {
        TypeDescriptor::Declared(declared) => Some(declared.id.constructor()),
        TypeDescriptor::Named(name) => {
            let _ = name;
            None
        }
        _ => None,
    }
}

fn collect_trait_implementations(
    module_id: crate::ModuleId,
    program: &Program,
    trait_ids: &BTreeMap<String, crate::TraitId>,
    external_interfaces: &BTreeMap<String, ModuleInterface>,
    contracts: &HashMap<String, TypeDescriptor>,
    schemes: &HashMap<String, TypeScheme>,
    sources: &SourceDatabase,
) -> Result<Vec<TraitImplementation>, FrontendError> {
    let known_traits = trait_ids
        .values()
        .copied()
        .chain(
            external_interfaces
                .values()
                .flat_map(|interface| interface.traits.values().copied()),
        )
        .collect::<HashSet<_>>();
    let mut implementations: Vec<TraitImplementation> = Vec::new();
    for (index, binding) in program
        .value
        .body
        .value
        .bindings
        .iter()
        .filter(|binding| binding.value.kind == BindingKind::Impl)
        .enumerate()
    {
        let contract = contracts
            .get(&binding.value.name.value)
            .expect("impl contract is evaluated before registry construction");
        let TypeDescriptor::Declared(dictionary) = contract else {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    "impl target must name a trait",
                    binding
                        .value
                        .annotation
                        .as_ref()
                        .expect("impl has an annotation")
                        .location,
                ),
            ));
        };
        let trait_id = crate::TraitId::from(dictionary.id.constructor());
        if !known_traits.contains(&trait_id) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    "impl target is not a visible trait",
                    binding
                        .value
                        .annotation
                        .as_ref()
                        .expect("impl has an annotation")
                        .location,
                ),
            ));
        }
        let [target] = dictionary.id.arguments() else {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    "trait implementation requires exactly one target type",
                    binding.location,
                ),
            ));
        };
        let target_owner = outer_nominal_constructor(target).map(|id| id.module);
        if trait_id.module != module_id && target_owner != Some(module_id) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error(
                    "orphan impl must be declared by the trait or target type provider",
                    binding.location,
                ),
            ));
        }
        let target_id = TypeExprId::from_descriptor(target);
        if let Some(previous) = implementations.iter().find(|implementation| {
            implementation.trait_id == trait_id
                && TypeExprId::from_descriptor(&implementation.target) == target_id
        }) {
            return Err(FrontendError::from_diagnostic(
                sources,
                Diagnostic::error("duplicate trait implementation", binding.location)
                    .with_secondary("first implementation", previous.location),
            ));
        }
        let parameters = schemes
            .get(&binding.value.name.value)
            .map(|scheme| scheme.parameters.clone())
            .unwrap_or_default();
        implementations.push(TraitImplementation {
            id: crate::TraitImplId {
                module: module_id,
                local: crate::FIRST_DYNAMIC_MODULE_LOCAL
                    .checked_add(u32::try_from(index).expect("impl count exceeds u32"))
                    .expect("impl slot exceeds u32"),
            },
            trait_id,
            target: target.clone(),
            dictionary: binding.value.name.value.clone(),
            parameters,
            location: binding.location,
        });
    }
    Ok(implementations)
}
