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
            || self.reference_instances.len() != self.hir.len()
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
