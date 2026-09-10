use super::*;
use crate::{source::Severity, type_image::TypeImage};

/// A read-only borrow of the solved graph and its detached type image. The
/// borrow prevents mutation while a consumer holds this publication capability.
/// Dropping it leaves the original MIR available to diagnostic/query consumers.
/// Identical full-build inputs must produce identical IDs and graph content,
/// independently of inventory enumeration order. Sealing never renumbers IDs.
pub struct SealedMir<'a> {
    mir: &'a Mir,
    types: TypeImage,
}

impl Mir {
    fn valid_properties(&self) -> bool {
        let mut keys = std::collections::BTreeSet::new();
        for record in &self.properties {
            if record.owner.index() >= self.types.len() || record.property.index() >= self.types.len()
                || record.providers.is_empty() || !keys.insert((record.owner, record.site, record.property)) {
                return false;
            }
            let mut pending = vec![record.owner, record.property];
            let mut seen = std::collections::BTreeSet::new();
            let mut concrete = true;
            while let Some(ty) = pending.pop() {
                if !seen.insert(ty) { continue; }
                let Some(ty) = self.types.get(ty.index()) else { return false; };
                concrete &= !matches!(ty.constructor, TypeConstructor::Parameter(_));
                pending.extend(ty.arguments.iter().copied());
            }
            if concrete != record.concrete { return false; }
            let instance = if let Some(id) = record.instance {
                let Some(instance) = self.generic_instances.get(id.index()) else { return false; };
                let Some(signature) = self.types.get(instance.signature.index()) else { return false; };
                if !instance.concrete || signature.constructor != TypeConstructor::Meta || signature.arguments != [record.owner] { return false; }
                Some(instance)
            } else { None };
            if record.providers.iter().any(|provider| {
                let ty = if let Some(instance) = instance { instance.ty(*provider) }
                    else { match self.ty_slots.get(provider.index()) { Some(TypeState::Known(ty)) => Some(*ty), _ => None } };
                ty != Some(record.property)
            }) { return false; }
            if let PropertySite::Field(index) | PropertySite::Variant(index) = record.site {
                let Some(layout) = self.type_layouts.get(record.owner.index()).and_then(Option::as_ref) else { return false; };
                if index as usize >= layout.members.len() || matches!(record.site, PropertySite::Field(_)) && layout.members[index as usize].is_none() { return false; }
            }
        }
        for template in self.properties.iter().filter(|record| !record.concrete && record.instance.is_none()) {
            let TypeConstructor::Nominal(symbol) = self.types[template.owner.index()].constructor else { return false; };
            for instance in self.generic_instances.iter().filter(|instance| instance.concrete && instance.symbol == symbol) {
                let signature = &self.types[instance.signature.index()];
                if signature.constructor != TypeConstructor::Meta || signature.arguments.len() != 1 { return false; }
                if !self.properties.iter().any(|record| record.concrete && record.owner == signature.arguments[0]
                    && record.site == template.site && record.providers == template.providers
                    && instance.ty(template.providers[0]) == Some(record.property)) { return false; }
            }
        }
        true
    }

    fn valid_interpreter_plan(&self, node: HirId) -> bool {
        let Some(plan) = self.interpreter_plans.get(node.index()).and_then(Option::as_ref) else { return false; };
        let Some(TypeState::Known(ty)) = self.ty_slots.get(node.index()) else { return false; };
        let outer = &self.types[ty.index()];
        if outer.constructor != TypeConstructor::Function || outer.arguments.len() != plan.witness_count as usize + 1 { return false; }
        if outer.arguments[..plan.witness_count as usize].iter().any(|ty| self.types[ty.index()].constructor != TypeConstructor::TypeOf) { return false; }
        let inner = &self.types[outer.arguments.last().unwrap().index()];
        if inner.constructor != TypeConstructor::Function || inner.arguments.len() != plan.parameters.len() + 1 { return false; }
        let Some(operand) = self.hir[node.index()].children.iter().find(|edge| edge.role == Role::Operand).map(|edge| edge.node) else { return false; };
        let Some(TypeState::Known(ty)) = self.ty_slots.get(operand.index()) else { return false; };
        let erased = &self.types[ty.index()];
        if erased.constructor != TypeConstructor::Function || erased.arguments.len() != inner.arguments.len()
            || (erased.arguments.last() != inner.arguments.last()
                && self.types[erased.arguments.last().unwrap().index()].constructor != TypeConstructor::Never) { return false; }
        plan.parameters.iter().enumerate().all(|(index, witness)| match witness {
            Some(witness) if *witness < plan.witness_count => {
                let witness = &self.types[outer.arguments[*witness as usize].index()];
                witness.arguments.first() == Some(&inner.arguments[index])
                    && self.types[erased.arguments[index].index()].constructor == TypeConstructor::Dyn
            }
            None => erased.arguments[index] == inner.arguments[index],
            _ => false,
        })
    }

    fn valid_newtype_selection(&self, node: HirId, ty: TypeId) -> bool {
        let (owner, payload) = match self.member_selections.get(node.index()) {
            Some(Some(MemberSelection::NewtypeConstructor)) => {
                let Some(signature) = self.types.get(ty.index()) else { return false; };
                if signature.constructor != TypeConstructor::Function || signature.arguments.len() != 2 { return false; }
                (signature.arguments[1], Some(signature.arguments[0]))
            }
            Some(Some(MemberSelection::NewtypePattern)) => (ty, None),
            _ => return true,
        };
        let Some(TypeConstructor::Nominal(symbol)) = self.types.get(owner.index()).map(|ty| &ty.constructor) else { return false; };
        self.type_definitions.iter().any(|definition| definition.symbol == *symbol && definition.operation == TypeOperation::Newtype)
            && self.type_layouts.get(owner.index()).and_then(Option::as_ref)
                .is_some_and(|layout| layout.members.len() == 1 && layout.members[0].is_some()
                    && payload.is_none_or(|payload| layout.members[0] == Some(payload)))
    }

    pub fn seal(&self) -> Result<SealedMir<'_>, Vec<Diagnostic>> {
        if !self.symbols_closed
            || !self.types_solved
            || !self.type_unknowns.is_empty()
            || !self.type_conflicts.is_empty()
            || self.types.iter().any(|ty| matches!(ty.constructor, TypeConstructor::ArrayLiteral | TypeConstructor::TupleLiteral))
            || self.reference_instances.len() != self.hir.len()
            || self.implementation_instances.len() != self.hir.len()
            || self.type_layouts.len() != self.types.len()
            || self.member_selections.len() != self.hir.len()
            || !self.valid_type_schemes()
            || !self.valid_properties()
            || !self.valid_property_admissions()
            || self.hir.iter().any(|node| matches!(node.kind, HirKind::LetElse)
                && node.children.iter().find(|edge| edge.role == Role::Else).is_none_or(|edge|
                    !matches!(self.ty_slots.get(edge.node.index()), Some(TypeState::Known(ty))
                        if self.types[ty.index()].constructor == TypeConstructor::Never)))
            || self.interpreter_plans.len() != self.hir.len()
            || self.hir.iter().enumerate().any(|(index, node)| matches!(node.kind, HirKind::Interpreter)
                && !self.valid_interpreter_plan(HirId(index as u32)))
            || self.member_selections.iter().enumerate().any(|(node, selection)| {
                if !matches!(selection, Some(MemberSelection::NewtypeConstructor | MemberSelection::NewtypePattern)) { return false; }
                let Some(TypeState::Known(ty)) = self.ty_slots.get(node) else { return true; };
                !self.valid_newtype_selection(HirId(node as u32), *ty)
            })
            || self.generic_instances.iter().any(|instance| instance.types.iter()
                .any(|(node, ty)| !self.valid_newtype_selection(*node, *ty)))
            || self.value_adjustments.len() != self.hir.len()
            || self.propagation_boundaries.len() != self.hir.len()
            || self.hir.iter().enumerate().any(|(node, hir)| matches!(hir.kind, HirKind::Propagate)
                && self.propagation_boundaries[node].is_none_or(|boundary| boundary.index() >= self.hir.len()))
            || self.generic_instances.iter().any(|instance| instance.types.iter().any(|(node, source)| {
                if self.value_adjustments.get(node.index()).is_none_or(Option::is_none) { return false; }
                let Some(target) = instance.adjustment(*node) else { return true; };
                self.types[source.index()].constructor != TypeConstructor::Unchecked || self.types[source.index()].arguments != [target]
            }))
            || self.value_adjustments.iter().enumerate().any(|(node, slot)| {
                let Some(slot) = slot else { return false; };
                let (Some(TypeState::Known(source)), Some(TypeState::Known(target))) = (self.ty_slots.get(node), self.ty_slots.get(slot.index())) else { return true; };
                self.types[source.index()].constructor != TypeConstructor::Unchecked || self.types[source.index()].arguments != [*target]
            })
            || self.construction_checks.iter().any(|check| {
                let signature = if let Some(instance) = check.instance {
                    self.generic_instances.get(instance.index()).filter(|instance| instance.concrete).and_then(|instance| instance.ty(check.checker))
                } else {
                    match self.ty_slots.get(check.checker.index()) { Some(TypeState::Known(ty)) => Some(*ty), _ => None }
                };
                check.owner.index() >= self.types.len() || check.signature.index() >= self.types.len() || signature != Some(check.signature)
            })
            || self.type_layouts.iter().flatten().any(|layout| layout.body.index() >= self.types.len() || layout.members.iter().flatten().any(|id| id.index() >= self.types.len()))
            || self.type_instances.iter().enumerate().any(|(node, arguments)| {
                !arguments.is_empty()
                    && (arguments.iter().any(|(_, slot)| !matches!(self.ty_slots[slot.index()], TypeState::Known(_)))
                        || self.reference_instances[node].is_none())
            })
            || self.bound_requirements.iter().any(|b| !b.state.is_proven())
            || self
                .diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
            || self
                .required_types
                .iter()
                .enumerate()
                .any(|(index, required)| {
                    *required && !matches!(self.ty_slots[index], TypeState::Known(_))
                })
        {
            return Err(vec![Diagnostic {
                severity: Severity::Error,
                message: "sealing requires a closed, valid MIR".into(),
                labels: vec![],
                notes: vec![],
            }]);
        }
        Ok(SealedMir {
            mir: self,
            types: TypeImage::from_mir(self)?,
        })
    }
}

impl<'a> SealedMir<'a> {
    pub fn mir(&self) -> &'a Mir {
        self.mir
    }
    pub fn types(&self) -> &TypeImage {
        &self.types
    }

    /// Transfer the image to the executable artifact without a second copy.
    pub(crate) fn into_parts(self) -> (&'a Mir, TypeImage) {
        (self.mir, self.types)
    }
}
