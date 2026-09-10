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
    pub fn seal(&self) -> Result<SealedMir<'_>, Vec<Diagnostic>> {
        if !self.symbols_closed
            || !self.types_solved
            || !self.type_unknowns.is_empty()
            || !self.type_conflicts.is_empty()
            || self.types.iter().any(|ty| matches!(ty.constructor, TypeConstructor::ArrayLiteral | TypeConstructor::TupleLiteral))
            || self.reference_instances.len() != self.hir.len()
            || self.implementation_instances.len() != self.hir.len()
            || self.type_layouts.len() != self.types.len()
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
