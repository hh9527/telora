use super::*;
use std::collections::BTreeMap;

impl Solver<'_> {
    pub(super) fn prepare_properties(&mut self) {
        for index in 0..self.mir.type_definitions.len() {
            let definition = &self.mir.type_definitions[index];
            let symbol = definition.symbol;
            let operation = definition.operation;
            let members = definition
                .members
                .iter()
                .map(|m| m.syntax)
                .collect::<Vec<_>>();
            for declaration in self.mir.symbols[symbol.index()].declarations.clone() {
                let Some(value) = self.child(declaration, Role::Value) else {
                    continue;
                };
                let Some(owner) = self.term(value.ty()).map(|t| t.arguments[0]) else {
                    continue;
                };
                self.attach_properties(declaration, owner, PropertySite::Type);
                for (index, member) in members.iter().enumerate() {
                    let site = if operation == TypeOperation::Enum {
                        PropertySite::Variant(index as u32)
                    } else {
                        PropertySite::Field(index as u32)
                    };
                    self.attach_properties(*member, owner, site);
                }
            }
        }
    }

    fn attach_properties(&mut self, syntax: HirId, owner: TypeSlotId, site: PropertySite) {
        for decorator in self.children(syntax, Role::Decorator) {
            if matches!(self.mir.hir[decorator.index()].kind, HirKind::ConstructionCheck { .. }) {
                self.attach_check(decorator, owner, site);
                continue;
            }
            let context = match site {
                PropertySite::Type => self.structure(TypeConstructor::Type, vec![]),
                PropertySite::Field(_) | PropertySite::Variant(_) => {
                    let mut fields = vec![];
                    for (name, constructor) in [
                        ("index", TypeConstructor::Int),
                        ("name", TypeConstructor::String),
                        ("owner", TypeConstructor::Type),
                    ] {
                        fields.push((name.to_owned(), self.structure(constructor, vec![])));
                    }
                    let ty = self.structure(TypeConstructor::Type, vec![]);
                    let field = if matches!(site, PropertySite::Variant(_)) {
                        (
                            "payload".to_owned(),
                            self.structure(TypeConstructor::Option, vec![ty]),
                        )
                    } else {
                        ("ty".to_owned(), ty)
                    };
                    fields.push(field);
                    fields.sort_by(|a, b| a.0.cmp(&b.0));
                    self.structure(
                        TypeConstructor::Record(fields.iter().map(|f| f.0.clone()).collect()),
                        fields.into_iter().map(|f| f.1).collect(),
                    )
                }
            };
            self.decorator_contexts[decorator.index()] = Some(context);
            self.property_declarations.push((owner, site, decorator));
        }
    }

    fn attach_check(&mut self, decorator: HirId, owner: TypeSlotId, site: PropertySite) {
        let Some(term) = self.term(owner).cloned() else { return };
        let TypeConstructor::Nominal(symbol) = term.constructor else { return };
        let Some((operation, members)) = self.nominal_members(symbol, &term.arguments) else { return };
        let input = match (operation, site) {
            (TypeOperation::Struct, PropertySite::Type) => self.structure(TypeConstructor::Unchecked, vec![owner]),
            (TypeOperation::Newtype, PropertySite::Type) => members[0].1.expect("newtype payload"),
            (TypeOperation::Enum, PropertySite::Variant(index)) => {
                let Some(payload) = members[index as usize].1 else { return }; payload
            }
            _ => return,
        };
        let arguments = self.children(decorator, Role::Argument);
        if arguments.len() != 1 { return; }
        if self.check_declarations.iter().any(|(other, other_site, _)| *other == owner && *other_site == site) {
            self.mir.diagnostics.push(Diagnostic::error("duplicate @check on the same construction boundary", self.mir.hir[decorator.index()].location));
        }
        let unit = self.structure(TypeConstructor::Tuple, vec![]);
        let blame = self.structure(TypeConstructor::Native(NativeTypeId::BLAME_ERROR), vec![]);
        let result = self.structure(TypeConstructor::Result, vec![unit, blame]);
        let signature = self.structure(TypeConstructor::Function, vec![input, result]);
        self.equal(arguments[0].ty(), signature, Some(self.mir.hir[decorator.index()].location));
        self.same(decorator, arguments[0].ty());
        self.check_declarations.push((owner, site, decorator));
    }

    pub(super) fn finalize_checks(&mut self) {
        let mut described = std::collections::BTreeSet::new();
        for &(owner, site, decorator) in &self.check_declarations {
            if let TypeState::Conflicted(id) = self.mir.ty_slots[decorator.ty().index()]
                && described.insert(id) {
                let conflict = &self.mir.type_conflicts[id.index()];
                // Preserve the diagnostic emitted with the original evidence.
                // A failed name resolution already has its own explanation.
                if conflict.resolve_origin.is_none()
                    && let Some(diagnostic) = self.mir.diagnostics.iter_mut().find(|diagnostic|
                        diagnostic.message == conflict.message
                            && diagnostic.labels.iter().any(|label| Some(label.location) == conflict.location)) {
                    diagnostic.message = format!("invalid @check function: {}; expected one construction input and Result((), BlameError)", diagnostic.message);
                    let location = self.mir.hir[decorator.index()].location;
                    if !diagnostic.labels.iter().any(|label| label.location == location) {
                        diagnostic.labels.push(crate::source::Label {
                            location, message: "@check contract required here".into(), primary: false,
                        });
                    }
                }
            }
            let (Some(owner), Some(signature)) = (self.known(owner), self.known(decorator.ty())) else { continue };
            let checker = self.child(decorator, Role::Argument).expect("check argument");
            let concrete = !self.contains_parameter(owner) && !self.contains_parameter(signature);
            self.mir.construction_checks.push(ConstructionCheck { owner, site, checker, signature, concrete, instance: None });
        }
    }

    pub(super) fn known(&self, slot: TypeSlotId) -> Option<TypeId> {
        match self.mir.ty_slots[slot.index()] {
            TypeState::Known(id) => Some(id),
            _ => None,
        }
    }

    pub(super) fn meta_type(&self, id: TypeId) -> Option<TypeId> {
        let ty = &self.mir.types[id.index()];
        (ty.constructor == TypeConstructor::Meta && ty.arguments.len() == 1)
            .then(|| ty.arguments[0])
    }

    pub(super) fn finalize_properties(&mut self) {
        let mut records = BTreeMap::<(TypeId, PropertySite, TypeId), Vec<HirId>>::new();
        for &(owner, site, provider) in &self.property_declarations {
            let (Some(owner), Some(property)) = (self.known(owner), self.known(provider.ty()))
            else {
                continue;
            };
            if !matches!(
                self.mir.types[property.index()].constructor,
                TypeConstructor::Nominal(_)
            ) {
                self.mir.diagnostics.push(Diagnostic::error(
                    "decorator result must be a nominal property type",
                    self.mir.hir[provider.index()].location,
                ));
                continue;
            }
            records
                .entry((owner, site, property))
                .or_default()
                .push(provider);
        }
        self.mir.properties = records
            .into_iter()
            .map(|((owner, site, property), providers)| PropertyRecord {
                owner,
                site,
                property,
                providers,
                concrete: !self.contains_parameter(owner) && !self.contains_parameter(property),
                instance: None,
                admission: None,
            })
            .collect();
    }

    pub(super) fn direct_evidence(&self, subject: TypeId, bound: TypeId) -> Option<BoundState> {
        if let TypeConstructor::Parameter(parameter) = self.mir.types[subject.index()].constructor {
            for &declaration in &self.mir.symbols[parameter.index()].declarations {
                for assumption in self.children(declaration, Role::Bound) {
                    if self.known(assumption.ty()) == Some(bound) {
                        return Some(BoundState::Assumed(parameter));
                    }
                }
            }
        }
        let raw = self.meta_type(bound)?;
        let ty = &self.mir.types[raw.index()];
        if ty.constructor != TypeConstructor::PropertyBound || ty.arguments.len() != 1 {
            return None;
        }
        let property = ty.arguments[0];
        for (index, fact) in self.mir.properties.iter().enumerate() {
            if fact.site != PropertySite::Type {
                continue;
            }
            let mut substitutions = BTreeMap::new();
            if self.match_type(fact.owner, subject, &mut substitutions)
                && self.match_type(fact.property, property, &mut substitutions)
            {
                return Some(BoundState::Property(index));
            }
        }
        Some(BoundState::Rejected)
    }

    /// Match a declaration skeleton, retaining equality of repeated parameters.
    /// This never binds an inference slot or manufactures evidence on failure.
    pub(super) fn match_type(
        &self,
        template: TypeId,
        actual: TypeId,
        substitutions: &mut BTreeMap<SymbolId, TypeId>,
    ) -> bool {
        let template = &self.mir.types[template.index()];
        if let TypeConstructor::Parameter(parameter) = template.constructor {
            return match substitutions.get(&parameter) {
                Some(&bound) => bound == actual,
                None => {
                    substitutions.insert(parameter, actual);
                    true
                }
            };
        }
        let actual = &self.mir.types[actual.index()];
        template.constructor == actual.constructor
            && template.arguments.len() == actual.arguments.len()
            && template
                .arguments
                .iter()
                .zip(&actual.arguments)
                .all(|(&t, &a)| self.match_type(t, a, substitutions))
    }
}
