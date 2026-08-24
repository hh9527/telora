#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeConstraint {
    pub parameter: TypeParameterId,
    pub capability: TypeCapability,
    pub location: crate::Location,
}

#[derive(Clone, Debug)]
pub enum TypeCapability {
    Trait { id: crate::TraitId, name: String },
    Property(TypeDescriptor),
}

impl PartialEq for TypeCapability {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Trait { id: left, .. }, Self::Trait { id: right, .. }) => left == right,
            (Self::Property(left), Self::Property(right)) => left == right,
            _ => false,
        }
    }
}

impl Eq for TypeCapability {}

impl TypeCapability {
    fn display_name(&self) -> String {
        match self {
            Self::Trait { name, .. } => name.clone(),
            Self::Property(property) => format!("Property({})", property.display_name()),
        }
    }
}

fn visible_trait(
    expression: &Expr,
    local: &BTreeMap<String, crate::TraitId>,
    external: &BTreeMap<String, ModuleInterface>,
) -> Option<(crate::TraitId, String)> {
    match &expression.value {
        ExprKind::Variable(name) => local
            .get(&name.value)
            .copied()
            .or_else(|| {
                external
                    .get(&name.value)
                    .and_then(|interface| interface.traits.get(&name.value))
                    .copied()
            })
            .map(|id| (id, name.value.clone())),
        ExprKind::Field { receiver, field } => {
            let ExprKind::Variable(namespace) = &receiver.value else {
                return None;
            };
            external
                .get(&namespace.value)
                .and_then(|interface| interface.traits.get(&field.value))
                .copied()
                .map(|id| (id, format!("{}.{}", namespace.value, field.value)))
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn evaluate_type_constraints(
    source_name: &str,
    parameters: &[TypeParameter],
    authored: &[Vec<Expr>],
    values: &BTreeMap<String, Val>,
    local_traits: &BTreeMap<String, crate::TraitId>,
    external_interfaces: &BTreeMap<String, ModuleInterface>,
    account: &mut QuotaAccount,
    sources: &SourceDatabase,
    evaluator: &mut ToolEvaluator<'_>,
) -> Result<Vec<TypeConstraint>, FrontendError> {
    let mut constraints = Vec::new();
    for (parameter, bounds) in parameters.iter().zip(authored) {
        for bound in bounds {
            let capability = if let Some((id, name)) =
                visible_trait(bound, local_traits, external_interfaces)
            {
                TypeCapability::Trait { id, name }
            } else if let ExprKind::Call { callee, arguments } = &bound.value
                && matches!(&callee.value, ExprKind::Variable(name) if name.value == "Property")
                && let [property] = arguments.as_slice()
            {
                let metadata = evaluate_tool_expression(
                    source_name,
                    property,
                    values,
                    account,
                    sources,
                    evaluator,
                )?;
                let descriptor = evaluator.decode_type(metadata, "Type").map_err(|message| {
                    FrontendError::from_diagnostic(
                        sources,
                        Diagnostic::error(
                            format!("invalid Property constraint: {message}"),
                            bound.location,
                        ),
                    )
                })?;
                TypeCapability::Property(descriptor)
            } else {
                return Err(FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error("unknown trait or constraint", bound.location),
                ));
            };
            if constraints.iter().any(|existing: &TypeConstraint| {
                existing.parameter == parameter.id && existing.capability == capability
            }) {
                return Err(FrontendError::from_diagnostic(
                    sources,
                    Diagnostic::error("duplicate type parameter constraint", bound.location),
                ));
            }
            constraints.push(TypeConstraint {
                parameter: parameter.id,
                capability,
                location: bound.location,
            });
        }
    }
    constraints.sort_by(|left, right| {
        left.parameter
            .cmp(&right.parameter)
            .then_with(|| left.capability.display_name().cmp(&right.capability.display_name()))
    });
    Ok(constraints)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraitImplementation {
    pub id: crate::TraitImplId,
    pub trait_id: crate::TraitId,
    pub target: TypeDescriptor,
    pub dictionary: String,
    pub dictionary_scheme: TypeScheme,
    pub parameters: Vec<TypeParameter>,
    pub constraints: Vec<TypeConstraint>,
    pub location: crate::Location,
}

#[derive(Clone, Debug)]
struct PendingTypeConstraint {
    capability: TypeCapability,
    target: TypeDescriptor,
    location: crate::Location,
    lexical_evidence: Vec<LexicalTypeEvidence>,
}

#[derive(Clone, Debug)]
struct LexicalTypeEvidence {
    capability: TypeCapability,
    target: TypeDescriptor,
    name: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedEvidence {
    pub binding: String,
    pub arguments: Vec<ResolvedEvidence>,
}

impl ResolvedEvidence {
    fn root(binding: String) -> Self {
        Self {
            binding,
            arguments: Vec::new(),
        }
    }
}

fn evidence_parameter_name(binding: &str, index: usize) -> String {
    format!("\0trait_evidence:{binding}:{index}")
}

fn trait_impl_runtime_name(id: crate::TraitImplId) -> String {
    format!("\0trait_impl:{}:{}", id.module.raw(), id.local)
}

fn published_trait_implementation(
    implementation: &TraitImplementation,
) -> TraitImplementation {
    let mut published = implementation.clone();
    published.dictionary = trait_impl_runtime_name(implementation.id);
    published
}

fn trait_implementations_overlap(
    left: &TraitImplementation,
    right: &TraitImplementation,
) -> bool {
    if left.trait_id != right.trait_id {
        return false;
    }
    let property_blanket_and_non_property_target =
        |blanket: &TraitImplementation, concrete: &TraitImplementation| {
            matches!(blanket.target, TypeDescriptor::Bound(_))
                && !blanket.constraints.is_empty()
                && blanket
                    .constraints
                    .iter()
                    .all(|constraint| matches!(constraint.capability, TypeCapability::Property(_)))
                && (matches!(
                    concrete.target,
                    TypeDescriptor::Int | TypeDescriptor::Float | TypeDescriptor::String
                ) || matches!(
                    concrete.target,
                    TypeDescriptor::Declared(DeclaredTypeDescriptor { ref name, .. })
                        if name.ends_with("__DisplayAtom")
                ))
        };
    if property_blanket_and_non_property_target(left, right)
        || property_blanket_and_non_property_target(right, left)
    {
        return false;
    }
    matches!(left.target, TypeDescriptor::Bound(_))
        || matches!(right.target, TypeDescriptor::Bound(_))
        || TypeExprId::from_descriptor(&left.target) == TypeExprId::from_descriptor(&right.target)
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
        let dictionary_scheme = schemes
            .get(&binding.value.name.value)
            .cloned()
            .expect("impl dictionary has a static scheme");
        let parameters = dictionary_scheme.parameters.clone();
        let constraints = dictionary_scheme.constraints.clone();
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
            dictionary_scheme,
            parameters,
            constraints,
            location: binding.location,
        });
    }
    Ok(implementations)
}

impl GenericInference<'_> {
    fn standalone_interpolation_type(target: &TypeDescriptor) -> bool {
        matches!(
            target,
            TypeDescriptor::Never
                | TypeDescriptor::Int
                | TypeDescriptor::Float
                | TypeDescriptor::String
                | TypeDescriptor::Atom(_)
        ) || matches!(target, TypeDescriptor::Enum(variants) if variants.values().all(Option::is_none))
    }

    fn require_interpolation_evidence(
        &mut self,
        target: TypeDescriptor,
        location: crate::Location,
    ) -> Result<(), String> {
        let target = self.resolve(&target);
        if contains_type_variable(&target) {
            self.pending_interpolations.push((target, location));
            return Ok(());
        }
        if matches!(target, TypeDescriptor::Never) {
            return Ok(());
        }
        if self.display_trait.is_none() && Self::standalone_interpolation_type(&target) {
            return Ok(());
        }
        let (trait_id, trait_name) = self.display_trait.clone().ok_or_else(|| {
            format!(
                "string interpolation of {} requires std/fmt.Display",
                target.display_name()
            )
        })?;
        let evidence = if let Some(binding) = self.lexical_trait_evidence(trait_id, &target) {
            ResolvedEvidence::root(binding)
        } else {
            let (implementation, replacements) = self
                .trait_candidate(trait_id, &target)?
                .ok_or_else(|| {
                    format!(
                        "type {} does not implement {trait_name}",
                        target.display_name()
                    )
                })?;
            let implementation = implementation.clone();
            self.implementation_evidence(&implementation, &replacements, location)?
        };
        self.resolved_interpolation_evidence
            .insert(location, evidence);
        Ok(())
    }

    fn finish_interpolations(&mut self) -> Result<(), (crate::Location, String)> {
        let pending = std::mem::take(&mut self.pending_interpolations);
        for (target, location) in pending {
            self.require_interpolation_evidence(target, location)
                .map_err(|message| (location, message))?;
        }
        if let Some((_, location)) = self.pending_interpolations.first() {
            return Err((
                *location,
                "string interpolation type remains unresolved".to_owned(),
            ));
        }
        Ok(())
    }

    fn runtime_type_evidence(
        &mut self,
        target: TypeDescriptor,
        location: crate::Location,
        index: usize,
    ) -> ResolvedEvidence {
        let binding = format!(
            "\0trait_target:{}:{}:{}",
            location.start, location.end, index
        );
        self.runtime_type_evidence
            .entry(binding.clone())
            .or_insert(target);
        ResolvedEvidence::root(binding)
    }

    fn push_lexical_evidence(&mut self, binding: &str, scheme: &TypeScheme) -> usize {
        let start = self.lexical_type_evidence.len();
        self.lexical_type_evidence.extend(
            scheme
                .constraints
                .iter()
                .enumerate()
                .map(|(index, constraint)| LexicalTypeEvidence {
                    capability: constraint.capability.clone(),
                    target: TypeDescriptor::Bound(constraint.parameter),
                    name: evidence_parameter_name(binding, index),
                }),
        );
        start
    }

    fn pop_lexical_evidence(&mut self, start: usize) {
        self.lexical_type_evidence.truncate(start);
    }

    fn lexical_trait_evidence(
        &self,
        trait_id: crate::TraitId,
        target: &TypeDescriptor,
    ) -> Option<String> {
        let target = self.resolve(target);
        self.lexical_type_evidence.iter().rev().find_map(|evidence| {
            matches!(
                &evidence.capability,
                TypeCapability::Trait { id, .. } if *id == trait_id
            )
            .then(|| (self.resolve(&evidence.target) == target).then(|| evidence.name.clone()))
            .flatten()
        })
    }

    fn lexical_property_evidence(
        &self,
        property: &TypeDescriptor,
        target: &TypeDescriptor,
    ) -> Option<String> {
        let target = self.resolve(target);
        self.lexical_type_evidence.iter().rev().find_map(|evidence| {
            matches!(
                &evidence.capability,
                TypeCapability::Property(candidate) if candidate == property
            )
            .then(|| (self.resolve(&evidence.target) == target).then(|| evidence.name.clone()))
            .flatten()
        })
    }

    fn published_property_evidence(
        &self,
        property: &TypeDescriptor,
        target: &TypeDescriptor,
    ) -> Option<String> {
        let target = self.resolve(target);
        self.type_properties.iter().find_map(|evidence| {
            (self.resolve(&evidence.target) == target && evidence.property == *property)
                .then(|| evidence.root.clone())
        })
    }

    fn implementation_evidence(
        &mut self,
        implementation: &TraitImplementation,
        replacements: &HashMap<TypeParameterId, TypeDescriptor>,
        location: crate::Location,
    ) -> Result<ResolvedEvidence, String> {
        if implementation.parameters.is_empty() {
            return Ok(ResolvedEvidence::root(implementation.dictionary.clone()));
        }
        let mut arguments = Vec::new();
        for (index, parameter) in implementation.parameters.iter().enumerate() {
            let target = replacements.get(&parameter.id).ok_or_else(|| {
                format!("blanket impl parameter {} is not determined", parameter.name)
            })?;
            arguments.push(self.runtime_type_evidence(
                self.resolve(target),
                location,
                index,
            ));
        }
        for constraint in &implementation.constraints {
            let target = replacements
                .get(&constraint.parameter)
                .map(|target| self.resolve(target))
                .ok_or_else(|| "blanket impl constraint target is not determined".to_owned())?;
            let evidence = match &constraint.capability {
                TypeCapability::Trait { id, name } => {
                    if let Some(binding) = self.lexical_trait_evidence(*id, &target) {
                        ResolvedEvidence::root(binding)
                    } else {
                        let (candidate, nested) = self
                            .trait_candidate(*id, &target)?
                            .ok_or_else(|| {
                                format!("type {} does not implement {name}", target.display_name())
                            })?;
                        let candidate = candidate.clone();
                        self.implementation_evidence(&candidate, &nested, location)?
                    }
                }
                TypeCapability::Property(property) => {
                    let binding = self
                        .lexical_property_evidence(property, &target)
                        .or_else(|| self.published_property_evidence(property, &target))
                        .ok_or_else(|| {
                            format!(
                                "type {} has no published Property({}) evidence",
                                target.display_name(),
                                property.display_name()
                            )
                        })?;
                    ResolvedEvidence::root(binding)
                }
            };
            arguments.push(evidence);
        }
        Ok(ResolvedEvidence {
            binding: implementation.dictionary.clone(),
            arguments,
        })
    }

    fn trait_dictionary_type(
        &self,
        trait_id: crate::TraitId,
        target: &TypeDescriptor,
    ) -> Option<TypeDescriptor> {
        let scheme = self
            .trait_ids
            .iter()
            .find_map(|(name, id)| (*id == trait_id).then(|| self.scheme(name)).flatten())
            .or_else(|| {
                self.external_interfaces.values().find_map(|interface| {
                    interface.traits.iter().find_map(|(name, id)| {
                        (*id == trait_id)
                            .then(|| interface.exports.get(name).cloned())
                            .flatten()
                    })
                })
            })?;
        let parameter = scheme.parameters.first()?.id;
        let body = substitute_bound_parameters(
            &scheme.body,
            &HashMap::from([(parameter, target.clone())]),
        );
        let TypeDescriptor::Function { result, .. } = body else {
            return None;
        };
        let TypeDescriptor::TypeOf(dictionary) = *result else {
            return None;
        };
        Some(*dictionary)
    }

    fn trait_member_reference(
        &self,
        callee: &Expr,
    ) -> Option<(crate::TraitId, String, String)> {
        let ExprKind::Field { receiver, field } = &callee.value else {
            return None;
        };
        match &receiver.value {
            ExprKind::Variable(name) => self
                .trait_ids
                .get(&name.value)
                .copied()
                .map(|id| (id, name.value.clone(), field.value.clone())),
            ExprKind::Field {
                receiver: namespace,
                field: trait_name,
            } => {
                let ExprKind::Variable(namespace) = &namespace.value else {
                    return None;
                };
                self.external_interfaces
                    .get(&namespace.value)
                    .and_then(|interface| interface.traits.get(&trait_name.value))
                    .copied()
                    .map(|id| {
                        (
                            id,
                            format!("{}.{}", namespace.value, trait_name.value),
                            field.value.clone(),
                        )
                    })
            }
            _ => None,
        }
    }

    fn infer_trait_call(
        &mut self,
        callee: &Expr,
        arguments: &[Expr],
        environment: &HashMap<String, TypeDescriptor>,
        expected: Option<&TypeDescriptor>,
    ) -> Option<Result<TypeDescriptor, String>> {
        let (trait_id, trait_name, member) = self.trait_member_reference(callee)?;
        Some((|| {
            let Some(first) = arguments.first() else {
                return Err(format!("{trait_name}.{member} requires a Self argument"));
            };
            let target = self.infer_authored_boundary(first, environment, None)?;
            let (dictionary, dictionary_type) =
                if let Some(dictionary) = self.lexical_trait_evidence(trait_id, &target) {
                    let dictionary_type = self
                        .trait_dictionary_type(trait_id, &target)
                        .ok_or_else(|| "trait has no static dictionary type".to_owned())?;
                    (ResolvedEvidence::root(dictionary), dictionary_type)
                } else {
                    let (implementation, replacements) = self
                        .trait_candidate(trait_id, &target)?
                        .ok_or_else(|| {
                            format!(
                                "type {} does not implement {trait_name}",
                                self.resolve(&target).display_name()
                            )
                        })?;
                    let implementation = implementation.clone();
                    let dictionary_type = if self
                        .display_trait
                        .as_ref()
                        .is_some_and(|(id, _)| *id == trait_id)
                        && matches!(self.resolve(&target), TypeDescriptor::Atom(_))
                        && implementation.id.module == trait_id.module
                        && matches!(implementation.target, TypeDescriptor::Declared(_))
                    {
                        self.trait_dictionary_type(trait_id, &target)
                            .ok_or_else(|| "trait has no static dictionary type".to_owned())?
                    } else {
                        substitute_bound_parameters(
                            &implementation.dictionary_scheme.body,
                            &replacements,
                        )
                    };
                    let evidence = self.implementation_evidence(
                        &implementation,
                        &replacements,
                        callee.location,
                    )?;
                    (evidence, dictionary_type)
                };
            let member_type = self.project_field(&dictionary_type, &member)?;
            let TypeDescriptor::Function { parameters, result } = member_type else {
                return Err(format!("trait member {trait_name}.{member} is not callable"));
            };
            if parameters.len() != arguments.len() {
                return Err(format!(
                    "trait member {trait_name}.{member} expects {} arguments, found {}",
                    parameters.len(),
                    arguments.len()
                ));
            }
            self.check(&target, &parameters[0])?;
            for (argument, parameter) in arguments.iter().skip(1).zip(parameters.iter().skip(1)) {
                self.infer_authored_boundary(argument, environment, Some(parameter))?;
            }
            if let Some(expected) = expected {
                self.check(&result, expected)?;
            }
            self.resolved_trait_members
                .insert(callee.location, dictionary);
            Ok(self.resolve(&result))
        })())
    }

    fn trait_candidate<'a>(
        &'a self,
        trait_id: crate::TraitId,
        target: &TypeDescriptor,
    ) -> Result<
        Option<(&'a TraitImplementation, HashMap<TypeParameterId, TypeDescriptor>)>,
        String,
    > {
        let target = self.resolve(target);
        if contains_type_variable(&target) {
            return Err(format!(
                "cannot resolve trait constraint for {}",
                target.display_name()
            ));
        }
        let mut candidates = Vec::new();
        for implementation in self
            .trait_implementations
            .iter()
            .filter(|implementation| implementation.trait_id == trait_id)
        {
            let mut replacements = HashMap::new();
            let matches = match &implementation.target {
                TypeDescriptor::Bound(parameter) => {
                    replacements.insert(*parameter, target.clone());
                    true
                }
                pattern => {
                    self.resolve(pattern) == target
                        || (self
                            .display_trait
                            .as_ref()
                            .is_some_and(|(id, _)| *id == trait_id)
                            && implementation.id.module == trait_id.module
                            && matches!(pattern, TypeDescriptor::Declared(_))
                            && matches!(target, TypeDescriptor::Atom(_)))
                }
            };
            if !matches {
                continue;
            }
            let satisfied = implementation.constraints.iter().all(|constraint| {
                let Some(argument) = replacements.get(&constraint.parameter) else {
                    return false;
                };
                match &constraint.capability {
                    TypeCapability::Trait { id, .. } => self
                        .trait_candidate(*id, argument)
                        .ok()
                        .flatten()
                        .is_some(),
                    TypeCapability::Property(property) => self
                        .lexical_property_evidence(property, argument)
                        .or_else(|| self.published_property_evidence(property, argument))
                        .is_some(),
                }
            });
            if satisfied {
                candidates.push((implementation, replacements));
            }
        }
        match candidates.as_slice() {
            [] => Ok(None),
            [candidate] => Ok(Some(candidate.clone())),
            _ => Err(format!(
                "ambiguous trait implementation for {}",
                target.display_name()
            )),
        }
    }

    fn finish_type_constraints(&mut self) -> Result<(), (crate::Location, String)> {
        let pending = std::mem::take(&mut self.pending_type_constraints);
        for constraint in pending {
            let target = self.resolve(&constraint.target);
            if contains_type_variable(&target) {
                continue;
            }
            if let Some(evidence) = constraint.lexical_evidence.iter().find(|evidence| {
                evidence.capability == constraint.capability
                    && self.resolve(&evidence.target) == target
            }) {
                self.resolved_call_evidence
                    .entry(constraint.location)
                    .or_default()
                    .push(ResolvedEvidence::root(evidence.name.clone()));
                continue;
            }
            match &constraint.capability {
                TypeCapability::Trait { id, name } => {
                    let (implementation, replacements) = self
                        .trait_candidate(*id, &target)
                        .map_err(|message| (constraint.location, message))?
                        .ok_or_else(|| {
                            (
                                constraint.location,
                                format!(
                                    "type {} does not implement {name}",
                                    target.display_name()
                                ),
                            )
                        })?;
                    let implementation = implementation.clone();
                    let dictionary = self
                        .implementation_evidence(
                            &implementation,
                            &replacements,
                            constraint.location,
                        )
                        .map_err(|message| (constraint.location, message))?;
                    self.resolved_call_evidence
                        .entry(constraint.location)
                        .or_default()
                        .push(dictionary);
                }
                TypeCapability::Property(property) => {
                    let evidence = self
                        .lexical_property_evidence(property, &target)
                        .or_else(|| self.published_property_evidence(property, &target))
                        .ok_or_else(|| {
                            (
                                constraint.location,
                                format!(
                                    "type {} has no published Property({}) evidence",
                                    self.resolve(&constraint.target).display_name(),
                                    property.display_name()
                                ),
                            )
                        })?;
                    self.resolved_call_evidence
                        .entry(constraint.location)
                        .or_default()
                        .push(ResolvedEvidence::root(evidence));
                }
            }
        }
        Ok(())
    }
}
